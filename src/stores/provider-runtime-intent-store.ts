import { create } from "zustand";

import type { AgentChatProviderKind } from "@/tauri/types";

type ProviderRuntimeIntent = Partial<Record<AgentChatProviderKind, true>>;

interface ProviderRuntimeIntentStore {
  providers: ProviderRuntimeIntent;
  observe: (provider: AgentChatProviderKind) => void;
  reset: () => void;
}

/**
 * Provider runtimes are intentionally gated behind interaction with a chat
 * pane. Restoring a persisted pane is application state, not permission to
 * spawn a long-lived CLI during startup.
 *
 * Intent is remembered per provider for the current authenticated shell. That
 * lets an intentionally-started session survive pane remounts without making
 * interaction with Claude implicitly authorize OpenCode, while auth identity
 * changes reset the gate before the next shell mounts.
 */
export const useProviderRuntimeIntent = create<ProviderRuntimeIntentStore>(
  (set) => ({
    providers: {},
    observe: (provider) =>
      set((state) =>
        state.providers[provider]
          ? state
          : { providers: { ...state.providers, [provider]: true } },
      ),
    reset: () => set({ providers: {} }),
  }),
);
