import { useEffect } from "react";
import { create } from "zustand";

import { listChatProviderCapabilities } from "@/tauri/commands";
import type {
  AgentChatProviderKind,
  ProviderChatCapabilities,
} from "@/tauri/types";

interface ProviderCapabilitiesStore {
  claude: ProviderChatCapabilities | null;
  codex: ProviderChatCapabilities | null;
  claudeError: string | null;
  codexError: string | null;
  loaded: boolean;
  refresh: (provider: AgentChatProviderKind) => Promise<void>;
  refreshAll: () => Promise<void>;
}

export const useProviderCapabilities = create<ProviderCapabilitiesStore>(
  (set) => ({
    claude: null,
    codex: null,
    claudeError: null,
    codexError: null,
    loaded: false,
    refresh: async (provider) => {
      try {
        const caps = await listChatProviderCapabilities(provider);
        set((state) =>
          provider === "claude"
            ? { ...state, claude: caps, claudeError: null }
            : { ...state, codex: caps, codexError: null },
        );
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        console.error(
          `[provider-capabilities] refresh(${provider}) failed:`,
          err,
        );
        set((state) =>
          provider === "claude"
            ? { ...state, claudeError: message }
            : { ...state, codexError: message },
        );
      }
    },
    refreshAll: async () => {
      const store = useProviderCapabilities.getState();
      await Promise.all([store.refresh("claude"), store.refresh("codex")]);
      set({ loaded: true });
    },
  }),
);

/**
 * Fetch chat-side provider capabilities once on mount. Mount exactly
 * once (e.g. in `App.tsx`). Subsequent refreshes happen implicitly when
 * the Rust side emits `provider_capabilities_updated` (live harvest
 * path, deferred — MVP ships fallback-only).
 */
export function useProviderCapabilitiesInit(): void {
  const refreshAll = useProviderCapabilities((s) => s.refreshAll);
  useEffect(() => {
    void refreshAll();
  }, [refreshAll]);
}

/** Convenience selector: capabilities for the given provider, or null. */
export function selectCapabilities(
  state: ProviderCapabilitiesStore,
  provider: AgentChatProviderKind,
): ProviderChatCapabilities | null {
  return provider === "claude" ? state.claude : state.codex;
}

/** Convenience selector: find a model by id within a provider's list. */
export function selectModel(
  caps: ProviderChatCapabilities | null,
  modelId: string | null | undefined,
) {
  if (!caps || !modelId) return null;
  return caps.models.find((m) => m.id === modelId) ?? null;
}
