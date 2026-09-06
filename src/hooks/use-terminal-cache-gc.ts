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
 *
 * Bundle note: `terminal-cache.ts` imports `@xterm/xterm` and its addons at
 * module scope. A static import here (and in `use-terminal-theme-sync.ts`)
 * dragged all of xterm into the eager entry chunk even though `TerminalPane`
 * — the only thing that actually constructs a terminal — is lazy-loaded. The
 * cache module is therefore only ever reached through a dynamic `import()`,
 * and only when the cache is enabled at all.
 */
import { useEffect } from "react";
import { useAppStore } from "@/stores/app-store";

/**
 * Flip to revive the persistent terminal cache's housekeeping (GC + theme
 * sync). While `false`, neither hook touches `terminal-cache.ts`, so xterm
 * stays out of the eager bundle and the hooks cost one effect each. Kept as
 * a plain constant rather than a feature flag on purpose: the cache itself
 * is dead code until `TerminalPane` is switched back to it, and a runtime
 * flag would suggest the toggle is safe to flip on its own.
 */
export const PERSISTENT_TERMINAL_CACHE_ENABLED = false;

/** Deferred so the module (and xterm behind it) loads on first use only. */
export function loadTerminalCache() {
  return import("@/components/terminal/terminal-cache");
}

const tracked = new Set<string>();

export function useTerminalCacheGc() {
  useEffect(() => {
    if (!PERSISTENT_TERMINAL_CACHE_ENABLED) return;
    // Seed from current state so we don't fire spurious disposals if the
    // cache somehow has stale entries on first mount.
    const initial = useAppStore.getState().appState?.terminal_sessions ?? [];
    for (const s of initial) tracked.add(s.session_id);

    return useAppStore.subscribe((state) => {
      const sessions = state.appState?.terminal_sessions;
      if (!sessions) return;
      const live = new Set(sessions.map((s) => s.session_id));
      const gone: string[] = [];
      for (const sid of tracked) {
        if (!live.has(sid)) {
          tracked.delete(sid);
          gone.push(sid);
        }
      }
      for (const sid of live) tracked.add(sid);
      if (gone.length === 0) return;
      // Sessions end rarely, so the one-time module load is paid here rather
      // than at app boot. Disposal is idempotent on an already-gone entry.
      void loadTerminalCache().then(({ disposeTerminal }) => {
        for (const sid of gone) disposeTerminal(sid);
      });
    });
  }, []);
}
