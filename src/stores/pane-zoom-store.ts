import { create } from "zustand";

/**
 * Which pane, if any, is zoomed to fill its surface (tmux's `prefix z`). Kept
 * per surface and only in this window: a zoom is a momentary view of the
 * split, not part of the layout the backend persists, so the split itself is
 * never rewritten.
 */
interface PaneZoomStore {
  zoomedPaneBySurface: Record<string, string>;
  /** Zoom `paneId`, or restore the split when it is already the zoomed one. */
  toggle: (surfaceId: string, paneId: string) => void;
  clear: (surfaceId: string) => void;
}

export const usePaneZoomStore = create<PaneZoomStore>((set) => ({
  zoomedPaneBySurface: {},
  toggle: (surfaceId, paneId) =>
    set((s) => {
      const next = { ...s.zoomedPaneBySurface };
      if (next[surfaceId] === paneId) delete next[surfaceId];
      else next[surfaceId] = paneId;
      return { zoomedPaneBySurface: next };
    }),
  clear: (surfaceId) =>
    set((s) => {
      if (!(surfaceId in s.zoomedPaneBySurface)) return s;
      const next = { ...s.zoomedPaneBySurface };
      delete next[surfaceId];
      return { zoomedPaneBySurface: next };
    }),
}));
