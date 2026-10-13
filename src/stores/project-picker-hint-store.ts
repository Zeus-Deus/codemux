import { create } from "zustand";
import { persist } from "zustand/middleware";

interface ProjectPickerHintStore {
  seen: boolean;
  markSeen: () => void;
}

/**
 * Per machine: the dotted "change project" hint on the new-chat headline
 * shows until the project picker has been opened once (see
 * `ProjectPickerHint`).
 */
export const useProjectPickerHintStore = create<ProjectPickerHintStore>()(
  persist(
    (set) => ({
      seen: false,
      markSeen: () => set({ seen: true }),
    }),
    { name: "codemux-project-picker-hint" },
  ),
);
