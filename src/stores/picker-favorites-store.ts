import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";

import type { AgentChatProviderKind } from "@/tauri/types";

/**
 * Step 12 Stage 6 — favorited models for the multi-provider picker.
 *
 * Persistence shape: a flat `string[]` of `${provider}::${modelId}`
 * keys, mirroring the testid + cmdk-value shape Stage 4 already
 * uses. Cross-provider keys never collide because the provider
 * driver is part of the key — favoriting `claude::claude-sonnet-4-6`
 * is independent of favoriting `opencode::anthropic/claude-sonnet-4-6`,
 * which is the right semantic (a user might want the Claude SDK
 * route AND the OpenCode-mediated Anthropic route to BOTH bubble up
 * in their picker).
 *
 * Storage strategy mirrors `mcp-store.ts` / `skills-store.ts`:
 * zustand `persist` middleware → localStorage under a versioned
 * key (`codemux:picker-favorites:v1`). The `partialize` selector
 * only persists the favorites array; transient fields (none today,
 * but future loading flags would live here) stay in-memory.
 *
 * Stale-favorite policy: a favorited model whose provider has gone
 * offline (e.g. OpenCode disconnected from anthropic, removing its
 * `anthropic/*` rows from the live capabilities) leaves its key in
 * storage. The picker filters by `caps.models` regardless of the
 * favorites set, so stale keys are simply ignored on next render.
 * If the provider reconnects, the favorite re-surfaces — the
 * intended behaviour for "drop credentials, swap them later"
 * workflows.
 */
interface PickerFavoritesState {
  /** Sorted array of `${provider}::${model_id}` keys. Sorted so
   *  serialised payloads are deterministic across reloads (the
   *  hydrate step would otherwise shuffle on whatever order the
   *  user toggled in). */
  favorites: string[];

  /** Toggle a (provider, model) entry. Idempotent in pairs — odd
   *  number of toggles flips state. */
  toggle: (provider: AgentChatProviderKind, modelId: string) => void;

  /** True iff the (provider, model) tuple is currently favorited. */
  isFavorite: (provider: AgentChatProviderKind, modelId: string) => boolean;

  /** Pure helper that builds the storage key without mutating state.
   *  Exported so tests + downstream selectors can match without
   *  duplicating the format string. */
  getKey: (provider: AgentChatProviderKind, modelId: string) => string;
}

const STORAGE_KEY = "codemux:picker-favorites:v1";

/** Shared key builder so the format is locked in one place. Matches
 *  the cmdk `value` shape (`${provider}::${slug}`) so a future
 *  feature mapping cmdk highlights → favorites doesn't need a
 *  separate translation step. */
export function pickerFavoriteKey(
  provider: AgentChatProviderKind,
  modelId: string,
): string {
  return `${provider}::${modelId}`;
}

export const usePickerFavorites = create<PickerFavoritesState>()(
  persist(
    (set, get) => ({
      favorites: [],

      getKey: (provider, modelId) => pickerFavoriteKey(provider, modelId),

      toggle: (provider, modelId) => {
        const key = pickerFavoriteKey(provider, modelId);
        const current = get().favorites;
        const next = current.includes(key)
          ? current.filter((k) => k !== key)
          : [...current, key].sort();
        set({ favorites: next });
      },

      isFavorite: (provider, modelId) => {
        const key = pickerFavoriteKey(provider, modelId);
        return get().favorites.includes(key);
      },
    }),
    {
      name: STORAGE_KEY,
      storage: createJSONStorage(() => localStorage),
      partialize: (s) => ({ favorites: s.favorites }),
    },
  ),
);
