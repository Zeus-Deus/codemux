import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";

/**
 * GUI-chrome title bar — opt-in pinned preset tiles.
 *
 * `PinnedPresetTiles` (title-bar.tsx) used to render every `chat_agent`
 * preset plus every `cli` preset with `preset.pinned` set — and nearly
 * every built-in preset ships `pinned: true` in `src-tauri/src/presets.rs`
 * (that flag means "show in the legacy PresetBar", a different concept).
 * The result: the title bar shipped flooded with tiles by default.
 *
 * This store is the frontend-only fix: a separate, user-controlled set of
 * preset ids that are pinned to the *title bar* specifically. Default is
 * EMPTY — a fresh install shows no tiles and no divider. Users opt in per
 * preset from the hover-revealed pin toggle in the `+` launcher
 * (agent-launcher.tsx).
 *
 * Persistence shape mirrors `picker-favorites-store.ts`: a flat, sorted
 * `string[]` of preset ids → zustand `persist` middleware → localStorage
 * under a versioned key (`codemux:titlebar-pins:v1`). Sorted so the
 * serialised payload is deterministic across reloads.
 */
interface TitlebarPinsState {
  /** Sorted array of preset ids pinned to the title bar. */
  pinnedIds: string[];

  /** Toggle a preset id. Idempotent in pairs — odd number of toggles
   *  flips membership. */
  toggleTitlebarPin: (presetId: string) => void;

  /** True iff the preset id is currently pinned to the title bar. */
  isTitlebarPinned: (presetId: string) => boolean;
}

const STORAGE_KEY = "codemux:titlebar-pins:v1";

export const useTitlebarPinsStore = create<TitlebarPinsState>()(
  persist(
    (set, get) => ({
      pinnedIds: [],

      toggleTitlebarPin: (presetId) => {
        const current = get().pinnedIds;
        const next = current.includes(presetId)
          ? current.filter((id) => id !== presetId)
          : [...current, presetId].sort();
        set({ pinnedIds: next });
      },

      isTitlebarPinned: (presetId) => get().pinnedIds.includes(presetId),
    }),
    {
      name: STORAGE_KEY,
      storage: createJSONStorage(() => localStorage),
      partialize: (s) => ({ pinnedIds: s.pinnedIds }),
    },
  ),
);
