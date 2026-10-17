import { useEffect, useRef } from "react";
import {
  splitPane,
  createTab,
  activateTab,
  activatePane,
  activateWorkspace,
  createEmptyWorkspace,
  runProjectDevCommand,
} from "@/tauri/commands";
import { launchAgentChatPane } from "@/lib/agent-chat/launch-pane";
import {
  reopenClosedTab,
  requestClosePane,
  requestCloseTab,
} from "@/lib/close-guard";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { RIGHT_PANEL_EMPTY, useUIStore } from "@/stores/ui-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { startNewHomeAgent } from "@/lib/agent-chat/new-home-agent";
import { openProjectFlow } from "@/hooks/use-project-actions";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import { normalizeKeyCombo } from "@/lib/keybind-utils";
import { escapeClaimedElsewhere } from "@/lib/escape-guard";
import { stepInterfaceSize } from "@/lib/typography";
import { announceInterfaceSize } from "@/components/overlays/interface-size-hud";
import { getRegistryEntry } from "@/lib/keybind-registry";
import { runPluginKeybind } from "@/contrib/keybinds";
import { updateAppShortcuts } from "@/lib/app-shortcuts";
import { isAppShortcutApplicable } from "@/lib/app-shortcut-applicability";
import { getJumpTarget } from "@/components/layout/sidebar-inbox-jump";
import { useSidebarViewStore } from "@/components/layout/sidebar-view-store";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import { selectWorkspaceNavigationTarget } from "@/lib/workspace-navigation";
import { jumpToNextNeedsYou } from "@/lib/needs-you";
import {
  findPaneInDirection,
  focusPaneContent,
  leafPaneIds,
  measurePaneRects,
  type PaneDirection,
} from "@/lib/pane-navigation";
import { usePaneZoomStore } from "@/stores/pane-zoom-store";
import {
  useSyncedSettingsStore,
  selectKeyboardShortcuts,
} from "@/stores/synced-settings-store";
import { reportFailure } from "@/lib/toast";

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

  // Keep app-shortcuts in sync with resolved keybinds (plugin keybinds
  // change `reverseMap` when a plugin turns on or off).
  useEffect(() => {
    updateAppShortcuts(overrides);
  }, [overrides, reverseMap]);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.repeat) return;
      if (recordingMode) return;

      const combo = normalizeKeyCombo(e);
      if (!combo) return;

      const actionIds = reverseMapRef.current.get(combo);
      if (!actionIds || actionIds.length === 0) return;

      // Find the first window-level action (skip actions a focused terminal or
      // chat composer handles itself)
      const actionId = actionIds.find((id) => {
        const entry = getRegistryEntry(id);
        return !entry || (entry.when !== "terminal" && entry.when !== "composer");
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
export function dispatch(actionId: string, event?: KeyboardEvent): boolean {
  if (!isAppShortcutApplicable(actionId)) return false;
  const ui = useUIStore.getState();
  const appState = useAppStore.getState().appState;

  // ── Close overlay (Escape) — conditional ──
  if (actionId === "closeOverlay") {
    // The shortcuts sheet layers over everything, Settings and onboarding
    // included, so it is the first layer to go. It runs before the
    // defaultPrevented guard below because the sheet's own dialog prevents
    // Escape so that this ladder, not Radix, closes it.
    if (ui.showShortcutsSheet) {
      ui.setShowShortcutsSheet(false);
      return true;
    }
    // A dialog or popover open on top of an overlay (Radix marks the Escape
    // it dismissed on) owns this press; closing the overlay beneath it too
    // would throw away the page the user was returning to.
    if (event?.defaultPrevented) return false;
    // Onboarding is a full-view replacement, not a modal — prioritize it over
    // dismissible overlays so Escape always provides an escape hatch.
    if (ui.onboardingProjectDir) {
      ui.setOnboardingProjectDir(null);
      return true;
    }
    if (ui.themeStudio) {
      ui.closeThemeStudio();
      return true;
    }
    if (ui.renameWorkspaceId) {
      ui.closeRenameWorkspace();
      return true;
    }
    if (ui.showSettings) {
      // Settings hosts sheets, selects, dialogs and its own command palette.
      // Escape belongs to the innermost of those; a second press leaves.
      if (event && escapeClaimedElsewhere(event)) return false;
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

  // ── Sidebar view (Activity ⇄ Projects) ──
  if (actionId === "toggleSidebarView") {
    useSidebarViewStore.getState().toggleView();
    return true;
  }

  // ── Interface size (zoom) ──
  // Steps the synced interface size rather than the webview zoom: every UI
  // size is rem-based, so this scales the app while native child surfaces
  // (browser panes, terminals) keep their real pixel geometry.
  if (actionId === "zoomIn" || actionId === "zoomOut" || actionId === "zoomReset") {
    const settings = useSyncedSettingsStore.getState();
    const current = settings.settings.appearance.interface_font_size;
    const step = actionId === "zoomIn" ? "in" : actionId === "zoomOut" ? "out" : "reset";
    const next = stepInterfaceSize(current, step);
    const changed = next !== current;
    if (changed) {
      void settings.updateSetting("appearance", "interface_font_size", next).catch(console.error);
    }
    announceInterfaceSize(next, step, changed);
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

  // ── New agent (mirrors the sidebar "New agent" button) ──
  // Works without an active workspace, so it sits before the appState guard.
  if (actionId === "newAgent") {
    const flags = useFeatureFlags.getState();
    if (!flags.enableAgentChat) {
      ui.setShowNewWorkspaceDialog(true);
      return true;
    }
    if (flags.enableLazyWorkspaceCreation) {
      startNewHomeAgent();
      return true;
    }
    ui.setShowNewWorkspaceDialog(true);
    return true;
  }

  // ── Open project (folder picker) ──
  if (actionId === "openProject") {
    openProjectFlow().catch(reportFailure("Couldn't open the project"));
    return true;
  }

  // ── Show keyboard shortcuts ──
  // A cheat sheet over the current view, so checking a binding never means
  // leaving the work. Pressing it again closes it.
  if (actionId === "showShortcuts") {
    ui.setShowShortcutsSheet(!ui.showShortcutsSheet);
    return true;
  }

  // ── Open the `+` launcher ──
  // Works on a draft too (its launcher has no workspace yet), so it sits
  // before the appState guard. Without a mounted launcher it does nothing.
  if (actionId === "openLauncher") {
    ui.requestOpenLauncher();
    return true;
  }

  // ── Workspaces ──
  if (actionId === "nextWorkspace" || actionId === "prevWorkspace") {
    const store = useAppStore.getState();
    const currentId = selectActiveWorkspaceId(store);
    const target = selectWorkspaceNavigationTarget(
      store.appState?.workspaces ?? [],
      currentId,
      actionId === "nextWorkspace" ? 1 : -1,
    );
    // A cycle that resolves to the already-selected workspace (single
    // workspace, or a wrap back onto the optimistic pending selection) is a
    // no-op: running the activation anyway would clear an open draft
    // composer and fire an IPC for a switch that never happens.
    if (target && target !== currentId) {
      activateWorkspaceInteraction(target).catch(console.error);
    }
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
    if (!target) return false;
    activateWorkspaceInteraction(target).catch(console.error);
    return true;
  }

  // ── Jump to the next workspace that needs you ──
  // The keyboard twin of the sidebar's "Needs you" strip, reachable from
  // anywhere, including with the sidebar collapsed.
  if (actionId === "jumpToNeedsYou") {
    return jumpToNextNeedsYou();
  }

  // Plugin keybinds (`plugin:<pluginId>:<localId>`) are global: they run
  // with or without an open workspace (a plugin checks host.workspace itself).
  if (actionId.startsWith("plugin:")) return runPluginKeybind(actionId, event);

  if (!appState) return false;
  const ws = appState.workspaces.find(
    (w) => w.workspace_id === selectActiveWorkspaceId(useAppStore.getState()),
  );
  if (!ws) return false;

  const surface = ws.surfaces.find(
    (s) => s.surface_id === ws.active_surface_id,
  );
  const activePaneId = surface?.active_pane_id;

  if (actionId === "runDevCommand") {
    runProjectDevCommand(ws.workspace_id).catch(reportFailure("Couldn't run the dev command"));
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
        await launchAgentChatPane(wsId, null, projectPath);
      } catch (err) {
        console.error("[shortcut] new workspace in project failed:", err);
        ui.setShowNewWorkspaceDialog(true, projectPath);
      }
    })();
    return true;
  }

  // ── Tabs ──
  if (actionId === "newTab") {
    createTab(ws.workspace_id, "terminal").catch(reportFailure("Couldn't open a terminal tab"));
    return true;
  }
  if (actionId === "closeTab") {
    if (ws.tabs.length > 0) void requestCloseTab(ws.workspace_id, ws.active_tab_id);
    return true;
  }
  if (actionId === "reopenClosedTab") {
    reopenClosedTab();
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

  // Next / previous tab, wrapping at either end
  if (actionId === "nextTab" || actionId === "prevTab") {
    const at = ws.tabs.findIndex((t) => t.tab_id === ws.active_tab_id);
    const step = actionId === "nextTab" ? 1 : -1;
    const next = ws.tabs[(at + step + ws.tabs.length) % ws.tabs.length];
    activateTab(ws.workspace_id, next.tab_id).catch(console.error);
    return true;
  }

  // ── Panes ──
  if (actionId === "splitPaneRight") {
    if (activePaneId) splitPane(activePaneId, "horizontal").catch(reportFailure("Couldn't split the pane"));
    return true;
  }
  if (actionId === "splitPaneDown") {
    if (activePaneId) splitPane(activePaneId, "vertical").catch(reportFailure("Couldn't split the pane"));
    return true;
  }
  if (actionId === "closePane") {
    if (activePaneId) void requestClosePane(activePaneId);
    return true;
  }

  // Move focus to the neighbouring pane on screen
  const focusMatch = actionId.match(/^focusPane(Left|Right|Up|Down)$/);
  if (focusMatch) {
    if (!surface || !activePaneId || surface.root.kind !== "split") return true;
    const direction = focusMatch[1].toLowerCase() as PaneDirection;
    const move = () => {
      const rects = measurePaneRects(leafPaneIds(surface.root));
      const active = rects.get(activePaneId);
      if (!active) return;
      const candidates = [...rects]
        .filter(([id]) => id !== activePaneId)
        .map(([id, rect]) => ({ id, rect }));
      const target = findPaneInDirection(active, candidates, direction);
      if (!target) return;
      activatePane(target).catch(console.error);
      focusPaneContent(target);
    };
    // A zoom hides the neighbours, so leave it first (as tmux does) and
    // measure once the split has painted again.
    const zoom = usePaneZoomStore.getState();
    if (zoom.zoomedPaneBySurface[surface.surface_id]) {
      zoom.clear(surface.surface_id);
      requestAnimationFrame(() => requestAnimationFrame(move));
    } else {
      move();
    }
    return true;
  }

  if (actionId === "togglePaneZoom") {
    if (!surface || !activePaneId) return false;
    usePaneZoomStore.getState().toggle(surface.surface_id, activePaneId);
    return true;
  }

  // Block browser reload shortcuts — returning true triggers preventDefault
  if (actionId === "blockReload" || actionId === "blockHardReload" || actionId === "blockF5Reload") {
    return true;
  }

  return false;
}
