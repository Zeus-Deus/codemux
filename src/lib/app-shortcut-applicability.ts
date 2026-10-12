import { getJumpTarget } from "@/components/layout/sidebar-inbox-jump";
import { needsYouWorkspaceIds, nextNeedsYouTarget } from "@/lib/needs-you";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useSidebarDensityStore } from "@/stores/sidebar-density-store";
import { useUIStore } from "@/stores/ui-store";

/** Check conditional actions without dispatching them, before a pane yields its input. */
export function isAppShortcutApplicable(actionId: string): boolean {
  if (actionId === "closeOverlay") {
    const ui = useUIStore.getState();
    return !!(ui.onboardingProjectDir || ui.themeStudio || ui.renameWorkspaceId ||
      ui.showSettings || ui.showFileSearch || ui.showContentSearch || ui.showCommandPalette);
  }

  const jump = actionId.match(/^workspaceJump([1-9])$/);
  if (jump) return getJumpTarget(Number(jump[1])) != null;

  const store = useAppStore.getState();
  if (actionId === "jumpToNeedsYou") {
    const ids = needsYouWorkspaceIds(
      store.appState?.workspaces ?? [],
      store.appState?.pane_statuses ?? {},
      useSidebarDensityStore.getState().statusSince,
    );
    return nextNeedsYouTarget(ids, selectActiveWorkspaceId(store)) != null;
  }

  if (actionId === "nextTab" || actionId === "prevTab" || actionId === "togglePaneZoom") {
    const workspace = store.appState?.workspaces.find(
      (ws) => ws.workspace_id === selectActiveWorkspaceId(store),
    );
    if (actionId === "nextTab" || actionId === "prevTab") {
      return (workspace?.tabs.length ?? 0) > 1;
    }
    const surface = workspace?.surfaces.find((s) => s.surface_id === workspace.active_surface_id);
    return !!surface?.active_pane_id && surface.root.kind === "split";
  }

  return true;
}
