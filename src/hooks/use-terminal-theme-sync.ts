/**
 * The persistent terminal cache is currently disabled, so this applies to an
 * empty cache. Keeping the subscription wired means a future cache revival
 * automatically follows the same app/system syntax-theme source as live panes.
 *
 * `terminal-cache.ts` is reached only through a dynamic `import()` and only
 * while the cache is enabled — a static import here put xterm in the eager
 * entry chunk (see `use-terminal-cache-gc.ts`). `xterm-theme.ts` is safe to
 * import statically: its only xterm dependency is the `ITheme` type, which
 * is erased at build time.
 */
import { useEffect } from "react";
import {
  getSyntaxThemeSnapshot,
  subscribeSyntaxTheme,
} from "@/hooks/use-theme-colors";
import { themeColorsToXtermTheme } from "@/lib/xterm-theme";
import {
  PERSISTENT_TERMINAL_CACHE_ENABLED,
  loadTerminalCache,
} from "@/hooks/use-terminal-cache-gc";

export function useTerminalThemeSync() {
  useEffect(() => {
    if (!PERSISTENT_TERMINAL_CACHE_ENABLED) return;
    let cancelled = false;
    // The theme is re-read at apply time (not captured at subscribe time) so
    // a change that lands while the module is still loading isn't applied
    // stale; the load itself is a one-off and cached by the module system.
    const apply = () => {
      void loadTerminalCache().then(({ applyThemeToAllTerminals }) => {
        if (cancelled) return;
        applyThemeToAllTerminals(themeColorsToXtermTheme(getSyntaxThemeSnapshot()));
      });
    };
    apply();
    const unsubscribe = subscribeSyntaxTheme(apply);
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);
}
