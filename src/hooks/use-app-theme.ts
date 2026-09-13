import { useMemo } from "react";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { useSettingsStore } from "@/stores/settings-store";
import { useOmarchyStore } from "@/stores/omarchy-store";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { initialThemeSource, THEME_SOURCE_KEY, type ThemeSource } from "@/lib/omarchy-theme";
import { parseCustomThemes, resolveTheme, THEME_BOOT_STORAGE_KEY } from "@/lib/themes";

const EMPTY: unknown[] = [];
// Capture before the first application writes its boot cache. A returning
// Graphite user is still a returning user, not an invitation to change themes.
const hadThemeAtStartup = (() => {
  try { return window.localStorage.getItem(THEME_BOOT_STORAGE_KEY) !== null; }
  catch { return false; }
})();

export function setThemeSource(source: ThemeSource) {
  if (!isRemoteClient()) useSettingsStore.getState().set(THEME_SOURCE_KEY, source);
}

/** One resolution rule for the shell, settings, picker and studio. */
export function useAppTheme() {
  const manualId = useSyncedSettingsStore((s) => s.settings?.appearance?.theme ?? "default");
  const payloads = useSyncedSettingsStore((s) => s.settings?.appearance?.custom_themes ?? EMPTY);
  const savedSource = useSettingsStore((s) => s.settings[THEME_SOURCE_KEY]);
  const legacyPalette = useSettingsStore((s) => s.settings["appearance.palette"]);
  const omarchy = useOmarchyStore((s) => s.theme);
  const discovered = useOmarchyStore((s) => s.loaded);
  const customThemes = useMemo(() => parseCustomThemes(payloads), [payloads]);
  const effectiveId = legacyPalette === "warm" && ["default", "system", "dark"].includes(manualId) ? "warm" : manualId;
  const manual = useMemo(() => resolveTheme(effectiveId, customThemes), [effectiveId, customThemes]);
  const source = isRemoteClient() ? "manual" : initialThemeSource({
    available: omarchy !== null,
    savedSource,
    hasPreviousTheme: hadThemeAtStartup,
    manualThemeId: manualId,
    legacyPalette,
  });
  return { theme: source === "omarchy" && omarchy ? omarchy : manual, omarchy, source, discovered, savedSource };
}
