import type { ThemeColors } from "@/tauri/types";
import { ANSI_SLOTS, contrastRatio, type AnsiPalette, type ThemeDefinition } from "./themes";

export const OMARCHY_THEME_ID = "omarchy";
export const THEME_SOURCE_KEY = "appearance.theme_source";
export type ThemeSource = "manual" | "omarchy";

/** Omarchy's own shell shades; any may be absent from a palette. */
export interface OmarchySurfaces {
  dark_background?: string | null;
  selection?: string | null;
  muted?: string | null;
}

export interface OmarchyTheme {
  name: string;
  scheme: "dark" | "light";
  colors: ThemeColors;
  surfaces?: OmarchySurfaces;
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

/** First candidate stepping from `preferred` toward `ink` that reads on every surface. */
function readableOn(surfaces: string[], preferred: string, ink: string): string {
  for (let step = 0; step <= 10; step++) {
    const candidate = mix(preferred, ink, step / 10);
    if (surfaces.every((surface) => contrastRatio(surface, candidate) >= 4.5)) return candidate;
  }
  return readable(surfaces[0], ink);
}

/**
 * Keep the desktop's palette and layer the shell the way Omarchy's own app
 * themes do: a recessed sidebar (`dark_background`), selection-colored
 * sidebar highlights, `muted` control outlines, and raised surfaces stepped
 * toward an accent-tinted foreground. Missing shades fall back to derived ones.
 */
export function omarchyToTheme({ name, scheme, colors: c, surfaces = {} }: OmarchyTheme): ThemeDefinition {
  const dark = scheme === "dark";
  const background = c.background;
  const foreground = readable(background, c.foreground);
  const ink = mix(foreground, c.accent, 0.3);
  const card = mix(background, ink, 0.05);
  const popover = mix(background, ink, 0.09);
  const secondary = mix(background, ink, 0.1);
  const muted = mix(background, ink, 0.16);
  // Omarchy's `muted` is a secondary *text* shade (vantablack sets #7a7a7a on
  // black), so as a structural hairline it reads as a heavy rule. Dividers
  // take a small fixed step off the canvas instead; inputs keep `muted` so
  // form controls stay clearly outlined.
  const border = mix(background, foreground, 0.08);
  const input = surfaces.muted ?? mix(background, foreground, 0.19);
  const accent = mix(background, c.accent, 0.18);
  const sidebar = surfaces.dark_background ?? mix(background, dark ? "#000000" : foreground, dark ? 0.25 : 0.05);
  const sidebarForeground = readableOn([sidebar], foreground, dark ? "#ffffff" : "#000000");
  const sidebarAccent = surfaces.selection ?? mix(sidebar, c.accent, 0.18);
  const mutedForeground = readableOn([background, sidebar], mix(background, foreground, 0.72), foreground);
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
      popover, popoverForeground: readable(popover, foreground),
      primary: c.accent, primaryForeground,
      secondary, secondaryForeground: readable(secondary, foreground),
      muted, mutedForeground,
      accent, accentForeground: readable(accent, foreground),
      border, input, ring: c.accent,
      sidebar, sidebarForeground,
      sidebarPrimary: c.accent, sidebarPrimaryForeground: primaryForeground,
      sidebarAccent, sidebarAccentForeground: readable(sidebarAccent, sidebarForeground),
      sidebarBorder: border, sidebarRing: c.accent, brandAccent: c.accent,
    },
  };
}
