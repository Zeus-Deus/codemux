import { useEffect, useRef } from "react";
import {
  splitPane,
  closePane,
  createTab,
  closeTab,
  cycleWorkspace,
  activateTab,
  activateWorkspace,
  createEmptyWorkspace,
  agentChatCreatePane,
  runProjectDevCommand,
} from "@/tauri/commands";
import { useAppStore } from "@/stores/app-store";
import { RIGHT_PANEL_EMPTY, useUIStore } from "@/stores/ui-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { openProjectFlow } from "@/hooks/use-project-actions";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import { normalizeKeyCombo } from "@/lib/keybind-utils";
import { getRegistryEntry } from "@/lib/keybind-registry";
import { updateAppShortcuts } from "@/lib/app-shortcuts";
import { getJumpTarget } from "@/components/layout/sidebar-inbox-jump";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
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

/**
 * Run a keybind action by id. The keyboard handler passes the originating
 * event; the command palette calls it without one, so a palette row and its
 * shortcut always execute the exact same code path.
 */
export function dispatch(actionId: string, _e?: KeyboardEvent): boolean {
  const ui = useUIStore.getState();
  const appState = useAppStore.getState().appState;

  // ── Close overlay (Escape) — conditional ──
  if (actionId === "closeOverlay") {
    // Onboarding is a full-view replacement, not a modal — prioritize it over
    // dismissible overlays so Escape always provides an escape hatch.
    if (ui.onboardingProjectDir) {
      ui.setOnboardingProjectDir(null);
      return true;
    }
    if (ui.renameWorkspaceId) {
      ui.closeRenameWorkspace();
      return true;
    }
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

  // ── New agent (mirrors the sidebar "New agent" + button) ──
  // Works without an active workspace, so it sits before the appState guard.
  if (actionId === "newAgent") {
    const flags = useFeatureFlags.getState();
    if (!flags.enableAgentChat) {
      ui.setShowNewWorkspaceDialog(true);
      return true;
    }
    if (flags.enableLazyWorkspaceCreation) {
      const store = useChatDraftStore.getState();
      // `lockedToHome` mirrors the sidebar button: a home-directory chat that
      // won't be redirected to whatever project is active in the sidebar.
      const draft = store.getOrCreateHomeDraft({ lockedToHome: true });
      store.setActiveDraft(draft.draftId);
      return true;
    }
    ui.setShowNewWorkspaceDialog(true);
    return true;
  }

  // ── Open project (folder picker) ──
  if (actionId === "openProject") {
    openProjectFlow().catch(console.error);
    return true;
  }

  // ── Show keyboard shortcuts ──
  if (actionId === "showShortcuts") {
    ui.setShowSettings(true, "shortcuts");
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

  // ── Jump to the Nth visible sidebar-inbox card ──
  // Resolves against the same filter-scoped, non-settled order the user sees,
  // and routes through the shared activation helper so the jump paints
  // optimistically and clears the draft exactly like the card's own click.
  // Runs before the appState guard so it works from anywhere.
  const jumpMatch = actionId.match(/^workspaceJump([1-9])$/);
  if (jumpMatch) {
    const target = getJumpTarget(parseInt(jumpMatch[1], 10));
    if (target) {
      activateWorkspaceInteraction(target).catch(console.error);
    }
    // Consume the combo regardless so a held Alt+digit never leaks to the page.
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

  if (actionId === "renameWorkspace") {
    ui.requestRenameWorkspace(ws.workspace_id);
    return true;
  }

  // ── Toggle right panel ──
  // True toggle: any open tab closes the panel; closed opens on the picker
  // sentinel, which resolves to whatever panes the deck already has instead
  // of force-opening Files (and silently clearing a Files dismissal).
  // Mirrors the titlebar cluster — using the store's by-tab-identity
  // `toggleRightPanel` would switch tabs instead of closing. Collapsing runs
  // through `collapseRightPanel`, the one path that releases a docked agent
  // browser: a collapsed panel is not a surface.
  if (actionId === "toggleRightPanel") {
    const current = ui.getRightPanelTab(ws.workspace_id);
    if (current == null) ui.setRightPanelTab(ws.workspace_id, RIGHT_PANEL_EMPTY);
    else ui.collapseRightPanel(ws.workspace_id);
    return true;
  }

  // ── New workspace in the current project (quick-create) ──
  // Mirrors the per-project "+" in the sidebar, scoped to the active
  // workspace's project. A home-rooted workspace just creates another home
  // workspace, which is the sensible "current context" there.
  if (actionId === "newWorkspaceInProject") {
    const projectPath = ws.project_root ?? ws.cwd;
    const flags = useFeatureFlags.getState();
    if (!flags.enableAgentChat) {
      ui.setShowNewWorkspaceDialog(true, projectPath);
      return true;
    }
    if (flags.enableLazyWorkspaceCreation) {
      const store = useChatDraftStore.getState();
      const draft = store.getOrCreateProjectDraft(projectPath);
      store.setActiveDraft(draft.draftId);
      return true;
    }
    void (async () => {
      try {
        const wsId = await createEmptyWorkspace(projectPath);
        await activateWorkspace(wsId);
        await agentChatCreatePane(wsId, null, projectPath);
      } catch (err) {
        console.error("[shortcut] new workspace in project failed:", err);
        ui.setShowNewWorkspaceDialog(true, projectPath);
      }
    })();
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
  if (actionId === "splitPaneDown") {
    if (activePaneId) splitPane(activePaneId, "vertical").catch(console.error);
    return true;
  }
  if (actionId === "closePane") {
    if (activePaneId) closePane(activePaneId).catch(console.error);
    return true;
  }

  // Block browser reload shortcuts — returning true triggers preventDefault
  if (actionId === "blockReload" || actionId === "blockHardReload" || actionId === "blockF5Reload") {
    return true;
  }

  return false;
}
