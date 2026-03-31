import { useEffect, useRef } from "react";
import {
  splitPane,
  closePane,
  createTab,
  closeTab,
  cycleWorkspace,
  activateTab,
  runProjectDevCommand,
} from "@/tauri/commands";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import { normalizeKeyCombo } from "@/lib/keybind-utils";
import { getRegistryEntry } from "@/lib/keybind-registry";
import { updateAppShortcuts } from "@/lib/app-shortcuts";
import {
  useSyncedSettingsStore,
  selectKeyboardShortcuts,
} from "@/stores/synced-settings-store";

/** Recording mode flag — set by keybind-editor to suppress dispatch */
let recordingMode = false;
export function setKeybindRecordingMode(active: boolean) {
  recordingMode = active;
}

export function useKeyboardShortcuts() {
  const { reverseMap } = useResolvedKeybinds();
  const overrides = useSyncedSettingsStore(selectKeyboardShortcuts);
  const reverseMapRef = useRef(reverseMap);
  reverseMapRef.current = reverseMap;

  // Keep app-shortcuts in sync with resolved keybinds
  useEffect(() => {
    updateAppShortcuts(overrides);
  }, [overrides]);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.repeat) return;
      if (recordingMode) return;

      const combo = normalizeKeyCombo(e);
      if (!combo) return;

      const actionIds = reverseMapRef.current.get(combo);
      if (!actionIds || actionIds.length === 0) return;

      // Find the first window-level action (skip terminal-only actions)
      const actionId = actionIds.find((id) => {
        const entry = getRegistryEntry(id);
        return !entry || entry.when !== "terminal";
      });
      if (!actionId) return;

      // Dispatch the action
      const dispatched = dispatch(actionId, e);
      if (dispatched) {
        e.preventDefault();
      }
    };

    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  return null;
}

function dispatch(actionId: string, _e: KeyboardEvent): boolean {
  const ui = useUIStore.getState();
  const appState = useAppStore.getState().appState;

  // ── Close overlay (Escape) — conditional ──
  if (actionId === "closeOverlay") {
    if (ui.showSettings) {
      ui.setShowSettings(false);
      return true;
    }
    if (ui.showFileSearch) {
      ui.setShowFileSearch(false);
      return true;
    }
    if (ui.showContentSearch) {
      ui.setShowContentSearch(false);
      return true;
    }
    if (ui.showCommandPalette) {
      ui.setShowCommandPalette(false);
      return true;
    }
    return false;
  }

  // ── Open settings ──
  if (actionId === "openSettings") {
    ui.setShowSettings(true);
    return true;
  }

  // ── Command palette ──
  if (actionId === "commandPalette") {
    ui.toggleCommandPalette();
    return true;
  }

  // ── Toggle sidebar ──
  if (actionId === "toggleSidebar") {
    ui.sidebarToggleFn?.();
    return true;
  }

  // ── Search ──
  if (actionId === "fileSearch") {
    ui.setShowFileSearch(true);
    return true;
  }
  if (actionId === "contentSearch") {
    ui.setShowContentSearch(true);
    return true;
  }

  // ── Workspaces ──
  if (actionId === "nextWorkspace") {
    cycleWorkspace(1).catch(console.error);
    return true;
  }
  if (actionId === "prevWorkspace") {
    cycleWorkspace(-1).catch(console.error);
    return true;
  }

  if (!appState) return false;
  const ws = appState.workspaces.find(
    (w) => w.workspace_id === appState.active_workspace_id,
  );
  if (!ws) return false;

  const surface = ws.surfaces.find(
    (s) => s.surface_id === ws.active_surface_id,
  );
  const activePaneId = surface?.active_pane_id;

  if (actionId === "runDevCommand") {
    runProjectDevCommand(ws.workspace_id).catch(console.error);
    return true;
  }

  // ── Tabs ──
  if (actionId === "newTab") {
    createTab(ws.workspace_id, "terminal").catch(console.error);
    return true;
  }
  if (actionId === "closeTab") {
    if (ws.tabs.length > 0)
      closeTab(ws.workspace_id, ws.active_tab_id).catch(console.error);
    return true;
  }

  // Switch to tab N
  const tabMatch = actionId.match(/^switchTab(\d)$/);
  if (tabMatch) {
    const idx = parseInt(tabMatch[1], 10) - 1;
    const tab = ws.tabs[idx];
    if (tab) activateTab(ws.workspace_id, tab.tab_id).catch(console.error);
    return true;
  }

  // ── Panes ──
  if (actionId === "splitPaneRight") {
    if (activePaneId) splitPane(activePaneId, "horizontal").catch(console.error);
    return true;
  }
  if (actionId === "closePane") {
    if (activePaneId) closePane(activePaneId).catch(console.error);
    return true;
  }

  return false;
}
