import { useMemo } from "react";
import { Button } from "@/components/ui/button";
import { parseCustomThemes } from "@/lib/themes";
import { useAppTheme } from "@/hooks/use-app-theme";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { useUIStore } from "@/stores/ui-store";
import { ThemeCoins, ThemeSchemeBadge } from "./theme-swatches";

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
  const { theme: activeTheme, source, omarchy } = useAppTheme();
  const customPayloads = useSyncedSettingsStore(
    (state) => state.settings?.appearance?.custom_themes ?? EMPTY_THEME_PAYLOADS,
  );
  const openCommandPaletteWith = useUIStore((state) => state.openCommandPaletteWith);
  const openThemeStudio = useUIStore((state) => state.openThemeStudio);

  const customThemes = useMemo(() => parseCustomThemes(customPayloads), [customPayloads]);

  const isCustom = customThemes.some((theme) => theme.id === activeTheme.id);

  return (
    <div className="flex items-center gap-4 rounded-lg border border-border/60 bg-muted/30 px-4 py-3.5">
      <ThemeCoins theme={activeTheme} size={34} />
      <div className="min-w-0 flex-1 space-y-0.5">
        <p className="flex items-center gap-1.5 text-body font-semibold text-foreground">
          <span className="truncate">{activeTheme.label}</span>
          <ThemeSchemeBadge scheme={activeTheme.scheme} />
        </p>
        <p className="text-body-sm text-muted-foreground/80">
          {source === "omarchy"
            ? omarchy ? "Following this desktop’s theme. Changes apply automatically." : "Omarchy is unavailable. Using your saved theme until it returns."
            : "Shell, terminal, code and editor. Synced to your account."}
        </p>
      </div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="flex-none"
        onClick={() => openCommandPaletteWith("theme")}
      >
        Change
        <span className="font-mono text-caption text-muted-foreground/70">⌘K</span>
      </Button>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="flex-none"
        onClick={() =>
          openThemeStudio(source === "omarchy" && omarchy
            ? { mode: "generate", copyTheme: activeTheme }
            : isCustom ? { editThemeId: activeTheme.id } : { mode: "generate" })
        }
      >
        {source === "omarchy" && omarchy ? "Customize a copy" : "Customize"}
      </Button>
    </div>
  );
}
