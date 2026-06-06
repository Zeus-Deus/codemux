import { useEffect } from "react";
import { create } from "zustand";

import type { LaunchModel } from "@/lib/launch-models";
import { listLaunchGeminiModels } from "@/tauri/commands";

/**
 * Gemini launch-time model list — backed by a backend hybrid harvest
 * (`list_launch_gemini_models`): live from Google's
 * `generativelanguage.googleapis.com/v1beta/models` when
 * `GEMINI_API_KEY` is set, otherwise the maintained fallback.
 *
 * The store keeps the fetched list in memory for the rest of the app's
 * lifetime — Gemini's model set is small and changes rarely, and the
 * launch picker reads from it on every open. Mounting the dialog
 * triggers a lazy first fetch via [`useLaunchGeminiModelsInit`].
 */
interface GeminiModelsState {
  models: LaunchModel[] | null;
  error: string | null;
  refresh: () => Promise<void>;
}

export const useLaunchGeminiModels = create<GeminiModelsState>((set) => ({
  models: null,
  error: null,
  refresh: async () => {
    try {
      const result = await listLaunchGeminiModels();
      set({
        models: result.map((m) => ({ id: m.id, label: m.label })),
        error: null,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      console.error("[gemini-models] refresh failed:", err);
      set({ error: message });
    }
  },
}));

/** Lazy-initialise: kicks a fetch on first mount, skips on subsequent
 *  mounts once `models` is populated. The Tauri command is cheap (the
 *  backend returns the maintained list synchronously when no API key
 *  is set), so deferring this to the first dialog open is fine. */
export function useLaunchGeminiModelsInit(): void {
  const refresh = useLaunchGeminiModels((s) => s.refresh);
  useEffect(() => {
    if (useLaunchGeminiModels.getState().models !== null) return;
    void refresh();
  }, [refresh]);
}
