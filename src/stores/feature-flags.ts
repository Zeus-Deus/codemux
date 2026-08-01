import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

import type { FeatureFlags } from "@/tauri/types";

interface FeatureFlagsStore {
  /** Whether the agent-chat pane kind and provider registry are enabled. */
  enableAgentChat: boolean;
  /** Whether sidebar-plus and boot-into-Home open a client-side chat
   *  draft instead of eagerly materialising a workspace. The draft is
   *  promoted to a real workspace on first message send. */
  enableLazyWorkspaceCreation: boolean;
  /** True once the initial Tauri call has resolved. Consumers should
   *  guard against rendering flag-dependent affordances until `loaded`
   *  flips, so a flash-of-content doesn't appear on startup. */
  loaded: boolean;
  /** Fetch the current flags from the backend. Safe to call more than
   *  once — later calls overwrite the in-memory state with whatever
   *  the backend currently reports. */
  refresh: () => Promise<void>;
  /** Atomic toggle for the Agent Chat GUI (default on; off = classic
   *  CLI interface). Flips `enableAgentChat` and
   *  `enableLazyWorkspaceCreation` together through the
   *  `set_agent_chat_enabled` Tauri command (which writes both fields
   *  under one mutex acquisition on the backend), then mirrors the
   *  new state into the store. The two flags must always move
   *  together — every production read site pairs them with `&&`, and
   *  the user-facing toggle in Settings → Interface is a single
   *  Switch. */
  setAgentChatEnabled: (enabled: boolean) => Promise<void>;
}

export const useFeatureFlags = create<FeatureFlagsStore>((set) => ({
  enableAgentChat: false,
  enableLazyWorkspaceCreation: false,
  loaded: false,
  refresh: async () => {
    try {
      const flags = await invoke<FeatureFlags>("get_feature_flags");
      set({
        enableAgentChat: flags.enable_agent_chat,
        enableLazyWorkspaceCreation: flags.enable_lazy_workspace_creation,
        loaded: true,
      });
    } catch (err) {
      console.error("Failed to fetch feature flags:", err);
      // Fall back to defaults-off on error so guards stay closed,
      // but mark loaded so consumers can stop waiting.
      set({ enableAgentChat: false, enableLazyWorkspaceCreation: false, loaded: true });
    }
  },
  setAgentChatEnabled: async (enabled: boolean) => {
    await invoke<void>("set_agent_chat_enabled", { enabled });
    set({
      enableAgentChat: enabled,
      enableLazyWorkspaceCreation: enabled,
    });
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
