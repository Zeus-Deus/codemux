import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

import type { FeatureFlags } from "@/tauri/types";

interface FeatureFlagsStore {
  /** Whether the agent-chat pane kind and provider registry are enabled. */
  enableAgentChat: boolean;
  /** True once the initial Tauri call has resolved. Consumers should
   *  guard against rendering flag-dependent affordances until `loaded`
   *  flips, so a flash-of-content doesn't appear on startup. */
  loaded: boolean;
  /** Fetch the current flags from the backend. Safe to call more than
   *  once — later calls overwrite the in-memory state with whatever
   *  the backend currently reports. */
  refresh: () => Promise<void>;
}

export const useFeatureFlags = create<FeatureFlagsStore>((set) => ({
  enableAgentChat: false,
  loaded: false,
  refresh: async () => {
    try {
      const flags = await invoke<FeatureFlags>("get_feature_flags");
      set({
        enableAgentChat: flags.enable_agent_chat,
        loaded: true,
      });
    } catch (err) {
      console.error("Failed to fetch feature flags:", err);
      // Fall back to defaults-off on error so guards stay closed,
      // but mark loaded so consumers can stop waiting.
      set({ enableAgentChat: false, loaded: true });
    }
  },
}));

/**
 * App-level hook that fetches feature flags once on mount. Mount
 * exactly once (e.g. in `App.tsx`) — the underlying Zustand store is
 * a singleton, so additional mounts would trigger redundant Tauri
 * calls without changing behavior.
 */
export function useFeatureFlagsInit() {
  const refresh = useFeatureFlags((s) => s.refresh);
  useEffect(() => {
    void refresh();
  }, [refresh]);
}
