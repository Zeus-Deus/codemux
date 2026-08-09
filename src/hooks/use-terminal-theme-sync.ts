/**
 * The persistent terminal cache is currently disabled, so this applies to an
 * empty cache. Keeping the subscription wired means a future cache revival
 * automatically follows the same app/system syntax-theme source as live panes.
 */
import { useEffect } from "react";
import { applyThemeToAllTerminals } from "@/components/terminal/terminal-cache";
import {
  getSyntaxThemeSnapshot,
  subscribeSyntaxTheme,
} from "@/hooks/use-theme-colors";
import { themeColorsToXtermTheme } from "@/lib/xterm-theme";

export function useTerminalThemeSync() {
  useEffect(() => {
    const apply = () => {
      applyThemeToAllTerminals(themeColorsToXtermTheme(getSyntaxThemeSnapshot()));
    };
    apply();
    return subscribeSyntaxTheme(apply);
  }, []);
}
