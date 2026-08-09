import { create } from "zustand";

/**
 * Peek-overlay open state for the GUI-mode background browser.
 *
 * A single `openWorkspaceId` (not a per-workspace map): the peek is a
 * transient "look at this now" affordance, so at most one can be open and
 * it must not silently re-open when the user later returns to a workspace
 * where it was left open. `BrowserPeekOverlay` additionally closes the peek
 * whenever the active workspace changes, so switching away always dismisses
 * it. Deliberately not persisted (unlike `rightPanelTabs` in `ui-store.ts`).
 */
interface BrowserPeekState {
  openWorkspaceId: string | null;
  isOpen: (workspaceId: string) => boolean;
  open: (workspaceId: string) => void;
  /** Close the peek if it is open for `workspaceId`; no-op otherwise. */
  close: (workspaceId: string) => void;
  /** Close the peek regardless of which workspace it is open for. */
  closeAll: () => void;
  toggle: (workspaceId: string) => void;
}

export const useBrowserPeekStore = create<BrowserPeekState>((set, get) => ({
  openWorkspaceId: null,
  isOpen: (workspaceId) => get().openWorkspaceId === workspaceId,
  open: (workspaceId) => set({ openWorkspaceId: workspaceId }),
  close: (workspaceId) =>
    set((s) =>
      s.openWorkspaceId === workspaceId ? { openWorkspaceId: null } : s,
    ),
  closeAll: () => set({ openWorkspaceId: null }),
  toggle: (workspaceId) =>
    set((s) => ({
      openWorkspaceId: s.openWorkspaceId === workspaceId ? null : workspaceId,
    })),
}));
