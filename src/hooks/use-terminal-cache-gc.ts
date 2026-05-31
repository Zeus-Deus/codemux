/**
 * DISABLED — operates on an empty cache. The persistent terminal cache in
 * `terminal-cache.ts` is not wired into the live app (see the banner there),
 * so its Map is never populated and this GC pass is a no-op every tick. Kept
 * wired in App.tsx alongside the cache for a possible future flag-gated revival.
 *
 * Garbage-collect the module-level terminal cache.
 *
 * The xterm Terminal for a session lives in `terminal-cache.ts` for the
 * full lifetime of the PTY session — workspace switches keep the cache
 * entry alive. The only truly session-ending events come from the Rust
 * backend (close_pane / close_tab / close_workspace, PTY exit, app
 * shutdown), which all manifest as the session_id disappearing from
 * AppState.terminal_sessions.
 *
 * This hook subscribes to that store and calls disposeTerminal for any
 * cached entry whose session is no longer tracked.
 */
import { useEffect } from "react";
import { useAppStore } from "@/stores/app-store";
import { disposeTerminal } from "@/components/terminal/terminal-cache";

const tracked = new Set<string>();

export function useTerminalCacheGc() {
  useEffect(() => {
    // Seed from current state so we don't fire spurious disposals if the
    // cache somehow has stale entries on first mount.
    const initial = useAppStore.getState().appState?.terminal_sessions ?? [];
    for (const s of initial) tracked.add(s.session_id);

    return useAppStore.subscribe((state) => {
      const sessions = state.appState?.terminal_sessions;
      if (!sessions) return;
      const live = new Set(sessions.map((s) => s.session_id));
      for (const sid of tracked) {
        if (!live.has(sid)) {
          tracked.delete(sid);
          disposeTerminal(sid);
        }
      }
      for (const sid of live) tracked.add(sid);
    });
  }, []);
}
