import { useActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";

/**
 * GUI chrome predicate (see `docs/features/gui-chrome.md`): renders for a
 * real, non-OpenFlow workspace when the Agent Chat Beta is on. A live
 * lazy-creation draft (no workspace yet) keeps the legacy chrome so the
 * draft surface's own PresetBar stays coherent; OpenFlow keeps its
 * dedicated chrome untouched.
 *
 * Extracted from `title-bar.tsx` (the original single call site) so other
 * GUI-mode-only surfaces — the background browser chip and context-bar
 * indicator (`docs/features/browser.md` "Background browser in GUI mode")
 * — gate on the exact same predicate without re-deriving it.
 */
export function useGuiChrome(): boolean {
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const lazyEnabled = useFeatureFlags((s) => s.enableLazyWorkspaceCreation);
  const activeDraftId = useChatDraftStore((s) => s.activeDraftId);
  const activeWorkspaceId = useActiveWorkspaceId();
  // Primitive selector — stays stable across backend ticks so consumers
  // don't re-render on every snapshot emit.
  const activeWorkspaceType = useAppStore((s) => {
    const id = s.appState?.active_workspace_id;
    if (!id) return null;
    return (
      s.appState!.workspaces.find((w) => w.workspace_id === id)
        ?.workspace_type ?? null
    );
  });

  const lazyDraftActive = lazyEnabled && activeDraftId !== null;
  return (
    enableAgentChat &&
    !lazyDraftActive &&
    activeWorkspaceId != null &&
    activeWorkspaceType != null &&
    activeWorkspaceType !== "open_flow"
  );
}
