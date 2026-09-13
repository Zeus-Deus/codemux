import type { ThemeColors } from "@/tauri/types";
import { ANSI_SLOTS, contrastRatio, type AnsiPalette, type ThemeDefinition } from "./themes";

export const OMARCHY_THEME_ID = "omarchy";
export const THEME_SOURCE_KEY = "appearance.theme_source";
export type ThemeSource = "manual" | "omarchy";

export interface OmarchyTheme {
  name: string;
  scheme: "dark" | "light";
  colors: ThemeColors;
}

export function initialThemeSource(options: {
  savedSource: string | undefined;
  hasPreviousTheme: boolean;
  manualThemeId: string;
  legacyPalette: string | undefined;
  available: boolean;
}): ThemeSource {
  if (options.savedSource === "omarchy" || options.savedSource === "manual") return options.savedSource;
  const untouched = ["default", "system", "dark"].includes(options.manualThemeId);
  return options.available && untouched && !options.hasPreviousTheme && !options.legacyPalette
    ? "omarchy" : "manual";
}

function mix(a: string, b: string, fraction: number): string {
  const channels = [1, 3, 5].map((offset) => {
    const x = Number.parseInt(a.slice(offset, offset + 2), 16);
    const y = Number.parseInt(b.slice(offset, offset + 2), 16);
    return Math.round(x + (y - x) * fraction).toString(16).padStart(2, "0");
  });
  return `#${channels.join("")}`;
}

function readable(background: string, preferred: string): string {
  if (contrastRatio(background, preferred) >= 4.5) return preferred;
  return contrastRatio(background, "#ffffff") > contrastRatio(background, "#000000") ? "#ffffff" : "#000000";
}

/** Keep the desktop's palette, deriving only the surfaces it doesn't specify. */
export function omarchyToTheme({ name, scheme, colors: c }: OmarchyTheme): ThemeDefinition {
  const background = c.background;
  const foreground = readable(background, c.foreground);
  const card = mix(background, foreground, 0.045);
  const secondary = mix(background, foreground, 0.085);
  const border = mix(background, foreground, 0.19);
  const accent = mix(background, c.accent, 0.18);
  const mutedForeground = readable(background, mix(background, foreground, 0.72));
  const primaryForeground = readable(c.accent, background);
  const ansi = Object.fromEntries(ANSI_SLOTS.map((slot, i) => [slot, c[`color${i}` as keyof ThemeColors]])) as unknown as AnsiPalette;
  return {
    id: OMARCHY_THEME_ID,
    label: `Omarchy · ${name}`,
    scheme,
    ansi,
    roles: {
      background, foreground,
      card, cardForeground: readable(card, foreground),
      popover: card, popoverForeground: readable(card, foreground),
      primary: c.accent, primaryForeground,
      secondary, secondaryForeground: readable(secondary, foreground),
      muted: secondary, mutedForeground,
      accent, accentForeground: readable(accent, foreground),
      border, input: border, ring: c.accent,
      sidebar: background, sidebarForeground: foreground,
      sidebarPrimary: c.accent, sidebarPrimaryForeground: primaryForeground,
      sidebarAccent: accent, sidebarAccentForeground: readable(accent, foreground),
      sidebarBorder: border, sidebarRing: c.accent, brandAccent: c.accent,
    },
  };
}
