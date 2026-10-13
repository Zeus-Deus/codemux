import { create } from "zustand";
import { persist } from "zustand/middleware";

interface FullAccessNoticeStore {
  dismissed: boolean;
  dismiss: () => void;
}

/**
 * Per machine: the Full access explanation above a new chat's composer is
 * shown until it is acknowledged once (see `FullAccessNotice`).
 */
export const useFullAccessNoticeStore = create<FullAccessNoticeStore>()(
  persist(
    (set) => ({
      dismissed: false,
      dismiss: () => set({ dismissed: true }),
    }),
    { name: "codemux-full-access-notice" },
  ),
);
