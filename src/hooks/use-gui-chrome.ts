import { useActiveWorkspaceId } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";

/**
 * GUI chrome predicate (see `docs/features/gui-chrome.md`): renders for a
 * real workspace when the Agent Chat Beta is on. A live
 * lazy-creation draft (no workspace yet) resolves `false` here — the
 * draft renders its own GUI-styled titlebar variant instead (see
 * `useDraftGuiChrome` below), and workspace-scoped GUI surfaces (the
 * background browser chip, terminal-header indicator, titlebar tabs) must
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
