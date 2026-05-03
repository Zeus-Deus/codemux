import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";

import {
  MCP_CODEMUX_SELF_ID,
  setMcpDisabledIds,
} from "@/tauri/commands";

/**
 * MCP server enable/disable preferences. Persisted to localStorage so
 * toggles survive restarts; mirrored into the Rust runtime via
 * {@link setMcpDisabledIds} on every change so the spawn pipeline
 * respects them on the next prime.
 *
 * Codemux's hardcoded MCP (id `"codemux-self"`) cannot be disabled —
 * the toggle action filters it out and the Settings UI hides the
 * switch on its row.
 */
interface McpState {
  /** Server ids the user has explicitly disabled. Stored sorted for
   *  deterministic localStorage payloads (so equality checks across
   *  restarts don't churn). */
  disabledIds: string[];

  /** Toggle a server's disabled state. Idempotent — calling twice is
   *  a no-op net of UI updates. */
  toggleDisabled: (id: string) => void;

  /** Whether `id` is currently disabled. */
  isDisabled: (id: string) => boolean;

  /** Push the current disabledIds set into the Rust runtime. The store
   *  calls this automatically after every mutation; callers can also
   *  call it on App mount to ensure the runtime starts in sync. */
  syncToBackend: () => Promise<void>;
}

const STORAGE_KEY = "codemux:mcp:v1";

export const useMcpStore = create<McpState>()(
  persist(
    (set, get) => ({
      disabledIds: [],

      toggleDisabled: (id) => {
        // Codemux self can't be disabled — guard at the action so a
        // mistaken caller (test / devtools) can't put us in a state
        // the runtime would reject anyway.
        if (id === MCP_CODEMUX_SELF_ID) return;

        const current = get().disabledIds;
        const next = current.includes(id)
          ? current.filter((x) => x !== id)
          : [...current, id].sort();

        set({ disabledIds: next });
        // Fire-and-forget sync — the Rust side's `set_disabled_ids`
        // tolerates frequent calls and is the only path that stops
        // already-running servers when the user disables them.
        void setMcpDisabledIds(next).catch((err) =>
          console.warn("[mcp] setMcpDisabledIds failed:", err),
        );
      },

      isDisabled: (id) => {
        if (id === MCP_CODEMUX_SELF_ID) return false;
        return get().disabledIds.includes(id);
      },

      syncToBackend: async () => {
        try {
          await setMcpDisabledIds(get().disabledIds);
        } catch (err) {
          console.warn("[mcp] setMcpDisabledIds (sync) failed:", err);
        }
      },
    }),
    {
      name: STORAGE_KEY,
      storage: createJSONStorage(() => localStorage),
      partialize: (s) => ({ disabledIds: s.disabledIds }),
    },
  ),
);
