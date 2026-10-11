import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";

export interface PeekSize {
  width: number;
  height: number;
}

export const DEFAULT_PEEK_SIZE: PeekSize = { width: 440, height: 300 };
export const MIN_PEEK_SIZE: PeekSize = { width: 360, height: 240 };
/** Largest share of the window the peek may cover on either axis. */
export const MAX_PEEK_FRACTION = 0.6;

/** Keep a peek size readable but never larger than 60% of the window. */
export function clampPeekSize(size: PeekSize, viewport: PeekSize): PeekSize {
  const clamp = (value: number, min: number, max: number) =>
    Math.round(Math.min(Math.max(value, min), Math.max(min, max)));
  return {
    width: clamp(size.width, MIN_PEEK_SIZE.width, viewport.width * MAX_PEEK_FRACTION),
    height: clamp(size.height, MIN_PEEK_SIZE.height, viewport.height * MAX_PEEK_FRACTION),
  };
}

function validPeekSize(value: unknown): PeekSize {
  const v = value as Partial<PeekSize> | null | undefined;
  return typeof v?.width === "number" &&
    typeof v.height === "number" &&
    Number.isFinite(v.width) &&
    Number.isFinite(v.height)
    ? { width: v.width, height: v.height }
    : DEFAULT_PEEK_SIZE;
}

/**
 * Peek-overlay state for the GUI-mode background browser.
 *
 * A single `openWorkspaceId` (not a per-workspace map): the peek is a
 * transient "look at this now" affordance, so at most one can be open and
 * it must not silently re-open when the user later returns to a workspace
 * where it was left open. `BrowserPeekOverlay` additionally closes the peek
 * whenever the active workspace changes, so switching away always dismisses
 * it. Only the user's chosen `size` is persisted; open state never is.
 */
interface BrowserPeekState {
  openWorkspaceId: string | null;
  size: PeekSize;
  isOpen: (workspaceId: string) => boolean;
  open: (workspaceId: string) => void;
  /** Close the peek if it is open for `workspaceId`; no-op otherwise. */
  close: (workspaceId: string) => void;
  /** Close the peek regardless of which workspace it is open for. */
  closeAll: () => void;
  toggle: (workspaceId: string) => void;
  setSize: (size: PeekSize) => void;
}

export const useBrowserPeekStore = create<BrowserPeekState>()(
  persist(
    (set, get) => ({
      openWorkspaceId: null,
      size: DEFAULT_PEEK_SIZE,
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
      setSize: (size) => set({ size }),
    }),
    {
      name: "codemux:browser-peek",
      version: 1,
      storage: createJSONStorage(() => ({
        getItem: (key) => {
          try {
            return localStorage.getItem(key);
          } catch {
            return null;
          }
        },
        setItem: (key, value) => {
          try {
            localStorage.setItem(key, value);
          } catch {
            /* Keep working when browser storage is unavailable. */
          }
        },
        removeItem: (key) => {
          try {
            localStorage.removeItem(key);
          } catch {
            /* No storage. */
          }
        },
      })),
      partialize: (state) => ({ size: state.size }),
      merge: (persisted, current) => ({
        ...current,
        size: validPeekSize((persisted as { size?: unknown } | null)?.size),
      }),
    },
  ),
);
