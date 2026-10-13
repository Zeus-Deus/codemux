import { useEffect } from "react";
import { create } from "zustand";

import { getPresetAvailability } from "@/tauri/commands";

interface PresetAvailabilityStore {
  /** Last answer from the backend, kept across launcher opens so a reopen
   *  renders the known state at once instead of flashing every row. */
  installed: Record<string, boolean>;
  refresh: () => Promise<void>;
}

export const usePresetAvailabilityStore = create<PresetAvailabilityStore>(
  (set) => ({
    installed: {},
    refresh: () =>
      getPresetAvailability()
        .then((installed) => set({ installed }))
        .catch((err) => {
          // Keep the last answer: a failed probe must never mark a working
          // agent as missing.
          console.warn("[preset-availability] probe failed:", err);
        }),
  }),
);

/**
 * Which CLI presets have their agent binary installed. Re-checks each time
 * `active` turns true (the launcher opening), so an agent installed while
 * Codemux is running shows up on the next open.
 */
export function usePresetAvailability(active: boolean): Record<string, boolean> {
  const installed = usePresetAvailabilityStore((s) => s.installed);
  const refresh = usePresetAvailabilityStore((s) => s.refresh);
  useEffect(() => {
    if (active) void refresh();
  }, [active, refresh]);
  return installed;
}

/** False only for a preset the backend confirmed is not installed; an
 *  unchecked preset counts as installed. */
export function isPresetInstalled(
  installed: Record<string, boolean>,
  presetId: string,
): boolean {
  return installed[presetId] !== false;
}
