import { useEffect, useRef } from "react";
import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { openHomeChat } from "@/lib/home-chat";
import { hasAnyPane } from "@/lib/pane-tree";

/**
 * Boot-into-Home: on first authenticated render after both the app
 * state snapshot and the feature flags have resolved, if the agent-chat
 * flag is ON and the restored session has nothing meaningful to show
 * (no active workspace, OR the active workspace's active surface has
 * no pane leaves), activate Home and drop the user into an empty chat
 * landing.
 *
 * Session-restore still wins whenever there's a real pane to resume —
 * a terminal, browser, or chat left open by the user takes priority
 * over Home.
 */
export function useBootIntoHome() {
  const appState = useAppStore((s) => s.appState);
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const flagsLoaded = useFeatureFlags((s) => s.loaded);
  const firedRef = useRef(false);

  useEffect(() => {
    if (firedRef.current) return;
    if (!appState || !flagsLoaded) return;
    if (!enableAgentChat) return;

    const activeId = appState.active_workspace_id;
    const activeWs = activeId
      ? appState.workspaces.find((w) => w.workspace_id === activeId)
      : null;

    if (activeWs) {
      const activeSurface = activeWs.surfaces.find(
        (s) => s.surface_id === activeWs.active_surface_id,
      );
      if (activeSurface && hasAnyPane(activeSurface.root)) {
        return;
      }
    }

    firedRef.current = true;
    openHomeChat().catch((err) => {
      console.error("[boot-into-home] failed:", err);
    });
  }, [appState, enableAgentChat, flagsLoaded]);
}
