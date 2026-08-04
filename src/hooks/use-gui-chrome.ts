import { useActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import type { PaneNodeSnapshot } from "@/tauri/types";

/**
 * GUI chrome predicate (see `docs/features/gui-chrome.md`): renders for a
 * real workspace when the Agent Chat Beta is on. A live
 * lazy-creation draft (no workspace yet) resolves `false` here — the
 * draft renders its own GUI-styled titlebar variant instead (see
 * `useDraftGuiChrome` below), and workspace-scoped GUI surfaces (the
 * background browser chip, context-bar indicator, titlebar tabs) must
 * stay off while the draft covers the workspace pane tree.
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
  const lazyDraftActive = lazyEnabled && activeDraftId !== null;
  return (
    enableAgentChat &&
    !lazyDraftActive &&
    activeWorkspaceId != null
  );
}

/**
 * Draft counterpart of `useGuiChrome`: true while the Agent Chat Beta
 * is on AND a lazy-creation draft is the active surface. Mutually
 * exclusive with `useGuiChrome` (whose `!lazyDraftActive` guard covers
 * exactly this window). Drives `TitleBar`'s GUI-styled draft variant —
 * the `h-10` bar with an "Agent Chat" pill and the draft agent
 * launcher — so a "+" new-workspace draft no longer flashes the legacy
 * `h-9` bar + `PresetBar` rows before the workspace materialises.
 */
export function useDraftGuiChrome(): boolean {
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const lazyEnabled = useFeatureFlags((s) => s.enableLazyWorkspaceCreation);
  // Require the draft ENTRY, not just the id — mirrors WorkspaceMain's
  // `activeDraftId && activeDraft` guard so the titlebar and the main
  // surface can never disagree about whether a draft is on screen
  // (e.g. a stale persisted `activeDraftId` whose entry was swept).
  // Primitive boolean selector, so backend ticks don't re-render.
  const hasActiveDraft = useChatDraftStore(
    (s) => s.activeDraftId !== null && s.draftsById[s.activeDraftId] != null,
  );
  return enableAgentChat && lazyEnabled && hasActiveDraft;
}

/** Recursively find the pane node with the given id under `node`,
 *  descending into `split` children. Mirrors `paneTreeContains` in
 *  `app-store.ts` (kept local — this is the only caller that needs
 *  the matched node itself, not just a boolean). */
function findPaneNode(
  node: PaneNodeSnapshot,
  paneId: string,
): PaneNodeSnapshot | null {
  if (node.pane_id === paneId) return node;
  if (node.kind === "split") {
    for (const child of node.children) {
      const found = findPaneNode(child, paneId);
      if (found) return found;
    }
  }
  return null;
}

/**
 * True when GUI chrome is active AND the active pane of the active
 * workspace's active surface is an Agent Chat pane. Drives
 * `WorkspaceContextBar`'s hide rule (`docs/features/workspace-context-bar.md`):
 * once an Agent Chat pane owns the surface, its own Context Row
 * (`docs/features/agent-chat.md` "Context Row") shows the same git/PR
 * detail inline under the composer, so the permanent bottom strip
 * would be redundant. A terminal (or other) pane active in GUI mode
 * keeps the bar — this only hides it for the agent-chat case.
 *
 * Legacy chrome (Beta flag off) always resolves `false` here because
 * `useGuiChrome()` does, leaving that mode's bar untouched.
 */
export function useAgentChatPaneActive(): boolean {
  const guiChrome = useGuiChrome();
  // Primitive-ish return (a pane `kind` string, or null) so this stays
  // stable across backend ticks that don't change the active pane —
  // same rationale as `useActiveWorkspaceBranch` et al. in app-store.ts.
  const activePaneKind = useAppStore((s) => {
    const id = s.appState?.active_workspace_id;
    if (!id) return null;
    const ws = s.appState!.workspaces.find((w) => w.workspace_id === id);
    if (!ws) return null;
    const surface = ws.surfaces.find(
      (sf) => sf.surface_id === ws.active_surface_id,
    );
    if (!surface) return null;
    return findPaneNode(surface.root, surface.active_pane_id)?.kind ?? null;
  });
  return guiChrome && activePaneKind === "agent_chat";
}
