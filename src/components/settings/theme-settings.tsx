import { useMemo } from "react";
import { Button } from "@/components/ui/button";
import { parseCustomThemes, resolveTheme } from "@/lib/themes";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { useUIStore } from "@/stores/ui-store";
import { ThemeCoins } from "./theme-swatches";

const EMPTY_THEME_PAYLOADS: unknown[] = [];

/**
 * Settings ▸ Appearance's theme row.
 *
 * The grid of theme cards that used to live here is gone: picking a theme
 * belongs in the command palette, where the whole app repaints behind the list
 * as you arrow through it. What is left states what is on and gets out of the
 * way — Change reopens the palette on the theme query, Customize opens the
 * studio on the current theme.
 */
export function ThemeSettings() {
  const themeId = useSyncedSettingsStore((state) => state.settings?.appearance?.theme ?? "default");
  const customPayloads = useSyncedSettingsStore(
    (state) => state.settings?.appearance?.custom_themes ?? EMPTY_THEME_PAYLOADS,
  );
  const openCommandPaletteWith = useUIStore((state) => state.openCommandPaletteWith);
  const openThemeStudio = useUIStore((state) => state.openThemeStudio);

  const customThemes = useMemo(() => parseCustomThemes(customPayloads), [customPayloads]);
  const activeTheme = useMemo(() => resolveTheme(themeId, customThemes), [themeId, customThemes]);
  const isCustom = customThemes.some((theme) => theme.id === activeTheme.id);

  return (
    <div className="flex items-center gap-4 rounded-xl border border-border/60 bg-muted/30 px-4 py-3.5">
      <ThemeCoins theme={activeTheme} size={34} />
      <div className="min-w-0 flex-1 space-y-0.5">
        <p className="truncate text-[13px] font-semibold text-foreground">{activeTheme.label}</p>
        <p className="text-[11.5px] text-muted-foreground/80">
          Shell, terminal, code and editor. Synced to your account.
        </p>
      </div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="h-[30px] flex-none gap-1.5 text-[11.5px]"
        onClick={() => openCommandPaletteWith("theme")}
      >
        Change
        <span className="font-mono text-[10px] text-muted-foreground/70">⌘K</span>
      </Button>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="h-[30px] flex-none text-[11.5px]"
        onClick={() =>
          openThemeStudio(isCustom ? { editThemeId: activeTheme.id } : { mode: "generate" })
        }
      >
        Customize
      </Button>
    </div>
  );
}
