import { useEffect } from "react";
import { create } from "zustand";

import { listChatProviderCapabilities } from "@/tauri/commands";
import { useFeatureFlags } from "@/stores/feature-flags";
import type {
  AgentChatProviderKind,
  ProviderChatCapabilities,
} from "@/tauri/types";

interface ProviderCapabilitiesStore {
  claude: ProviderChatCapabilities | null;
  codex: ProviderChatCapabilities | null;
  /** Step 12 Stage 3 — slot for OpenCode's live model harvest. Stays
   *  `null` until `refresh("opencode")` resolves. Failure surfaces in
   *  `opencodeError`; the slot itself stays `null` so the picker can
   *  render an empty state rather than a stale list. */
  opencode: ProviderChatCapabilities | null;
  claudeError: string | null;
  codexError: string | null;
  opencodeError: string | null;
  loaded: boolean;
  refresh: (provider: AgentChatProviderKind) => Promise<void>;
  refreshAll: () => Promise<void>;
}

export const useProviderCapabilities = create<ProviderCapabilitiesStore>(
  (set) => ({
    claude: null,
    codex: null,
    opencode: null,
    claudeError: null,
    codexError: null,
    opencodeError: null,
    loaded: false,
    refresh: async (provider) => {
      try {
        const caps = await listChatProviderCapabilities(provider);
        set((state) => storeOk(state, provider, caps));
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        console.error(
          `[provider-capabilities] refresh(${provider}) failed:`,
          err,
        );
        set((state) => storeErr(state, provider, message));
      }
    },
    refreshAll: async () => {
      const store = useProviderCapabilities.getState();
      // Promise.all is fine here — each `refresh` awaits its own
      // try/catch so a single provider's failure (commonly OpenCode
      // not installed) doesn't reject the whole barrier.
      await Promise.all([
        store.refresh("claude"),
        store.refresh("codex"),
        store.refresh("opencode"),
      ]);
      set({ loaded: true });
    },
  }),
);

/**
 * Fetch chat-side provider capabilities once on mount. Mount exactly
 * once (e.g. in `App.tsx`). Subsequent refreshes happen implicitly when
 * the Rust side emits `provider_capabilities_updated` (live harvest
 * path, deferred — MVP ships fallback-only for Claude/Codex; OpenCode
 * already does a live harvest at refresh time, see Stage 3).
 *
 * Step 13 — gates on `enableAgentChat`. When the master Beta toggle
 * is off this hook no-ops, so the picker never spawns
 * `opencode serve` or harvests `codex app-server` for a user who
 * hasn't opted in. The hook re-engages live (no remount required) the
 * moment the toggle flips on, because the flag is read from the
 * zustand store via the selector pattern.
 */
export function useProviderCapabilitiesInit(): void {
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const refreshAll = useProviderCapabilities((s) => s.refreshAll);
  useEffect(() => {
    if (!enableAgentChat) return;
    void refreshAll();
  }, [enableAgentChat, refreshAll]);
}

/** Convenience selector: capabilities for the given provider, or null.
 *  Exhaustive switch over [`AgentChatProviderKind`] — adding a fourth
 *  provider downstream now produces a TypeScript error here rather
 *  than a silent `null` fall-through. */
export function selectCapabilities(
  state: ProviderCapabilitiesStore,
  provider: AgentChatProviderKind,
): ProviderChatCapabilities | null {
  switch (provider) {
    case "claude":
      return state.claude;
    case "codex":
      return state.codex;
    case "opencode":
      return state.opencode;
  }
}

/** Convenience selector: error string for a given provider, or null. */
export function selectError(
  state: ProviderCapabilitiesStore,
  provider: AgentChatProviderKind,
): string | null {
  switch (provider) {
    case "claude":
      return state.claudeError;
    case "codex":
      return state.codexError;
    case "opencode":
      return state.opencodeError;
  }
}

/** Convenience selector: find a model by id within a provider's list. */
export function selectModel(
  caps: ProviderChatCapabilities | null,
  modelId: string | null | undefined,
) {
  if (!caps || !modelId) return null;
  return caps.models.find((m) => m.id === modelId) ?? null;
}

// ── Internal: per-provider state-update helpers ─────────────────────
//
// Pulled out of the inline `set()` callbacks so adding a fourth
// provider doesn't require ternary surgery in two places.

function storeOk(
  state: ProviderCapabilitiesStore,
  provider: AgentChatProviderKind,
  caps: ProviderChatCapabilities,
): Partial<ProviderCapabilitiesStore> {
  switch (provider) {
    case "claude":
      return { claude: caps, claudeError: null };
    case "codex":
      return { codex: caps, codexError: null };
    case "opencode":
      return { opencode: caps, opencodeError: null };
  }
  // Exhaustive switch above; the void return is unreachable but keeps
  // TS happy when AgentChatProviderKind grows.
  return state;
}

function storeErr(
  state: ProviderCapabilitiesStore,
  provider: AgentChatProviderKind,
  message: string,
): Partial<ProviderCapabilitiesStore> {
  switch (provider) {
    case "claude":
      return { claudeError: message };
    case "codex":
      return { codexError: message };
    case "opencode":
      return { opencodeError: message };
  }
  return state;
}
