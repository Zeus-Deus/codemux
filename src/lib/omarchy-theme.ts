import type { ThemeColors } from "@/tauri/types";
import {
  ANSI_SLOTS,
  DIVIDER_CONTRAST,
  SIDEBAR_DIVIDER_CONTRAST,
  contrastRatio,
  type AnsiPalette,
  type ThemeDefinition,
  hexToOklch,
  oklchToHex,
} from "./themes";

/** Loudest a control outline may get: clearly visible, never a heavy rule. */
const MAX_CONTROL_OUTLINE_CONTRAST = 2.5;

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

/** Nearest mix from `surface` toward `ink` that stands `target` off the surface. */
function dividerOn(surface: string, ink: string, target: number): string {
  for (let step = 1; step <= 100; step++) {
    const candidate = mix(surface, ink, step / 200);
    if (contrastRatio(candidate, surface) >= target) return candidate;
  }
  return mix(surface, ink, 0.5);
}

/** `color`, pulled back toward `surface` until it is no louder than `max`. */
function capContrast(color: string, surface: string, max: number): string {
  for (let step = 0; step <= 20; step++) {
    const candidate = mix(color, surface, step / 20);
    if (contrastRatio(candidate, surface) <= max) return candidate;
  }
  return surface;
}

/**
 * Keep the desktop's palette and layer the shell the way Omarchy's own T3 Code
 * theme does (`default/themed/t3code.json.tpl`): the desktop's exact canvas,
 * text, recessed sidebar (`dark_background`), and selection-colored sidebar
 * highlights, over raised surfaces generated the way T3 Code generates them.
 * Those surfaces climb an OKLCH lightness ramp off the canvas carrying the
 * *accent's* hue, so a warm accent on a cool canvas gives warm cards, menus,
 * and composer instead of a monochrome shell. Missing shades fall back to
 * derived ones.
 */
export function omarchyToTheme({ name, scheme, colors: c, surfaces = {} }: OmarchyTheme): ThemeDefinition {
  const dark = scheme === "dark";
  const background = c.background;
  const foreground = readable(background, c.foreground);
  const canvas = hexToOklch(background);
  const tone = hexToOklch(c.accent);
  // A neutral accent has no hue worth carrying; stay in the canvas's family.
  const hue = tone.C < 0.02 ? canvas.h : tone.h;
  const tint = Math.min(0.045, Math.max(0.008, tone.C * 0.22));
  const step = dark ? 1 : -1;
  const surfaceAt = (delta: number, chroma = tint) =>
    oklchToHex({ L: Math.min(0.98, Math.max(0.05, canvas.L + step * delta)), C: chroma, h: hue });
  const card = surfaceAt(0.05);
  const popover = surfaceAt(0.075);
  const secondary = surfaceAt(dark ? 0.1 : 0.06, Math.min(0.09, tone.C * 0.5));
  const muted = surfaceAt(dark ? 0.06 : 0.04, Math.min(0.06, tone.C * 0.35));
  const accent = surfaceAt(dark ? 0.13 : 0.08, Math.min(0.11, tone.C * 0.55));
  // Omarchy's `muted` is a secondary *text* shade (vantablack sets #7a7a7a on
  // black), so as a structural hairline it reads as a heavy rule. Dividers
  // stand a fixed contrast off the canvas instead; inputs keep `muted` so
  // form controls stay clearly outlined, capped so they never shout.
  const border = dividerOn(background, foreground, DIVIDER_CONTRAST);
  const input = capContrast(surfaces.muted ?? mix(background, foreground, 0.19), background, MAX_CONTROL_OUTLINE_CONTRAST);
  const sidebar = surfaces.dark_background ?? mix(background, dark ? "#000000" : foreground, dark ? 0.25 : 0.05);
  const sidebarForeground = readableOn([sidebar], foreground, dark ? "#ffffff" : "#000000");
  // Stepped off the sidebar itself: light palettes recess it *darker* than the
  // canvas, where a canvas-based hairline would vanish into it.
  const sidebarBorder = dividerOn(sidebar, foreground, SIDEBAR_DIVIDER_CONTRAST);
  const sidebarAccent = surfaces.selection ?? mix(sidebar, c.accent, 0.18);
  const mutedForeground = readableOn([background, sidebar], mix(background, foreground, 0.72), foreground);
  const primaryForeground = readable(c.accent, background);
  const ansi = Object.fromEntries(ANSI_SLOTS.map((slot, i) => [slot, c[`color${i}` as keyof ThemeColors]])) as unknown as AnsiPalette;
  return {
    id: OMARCHY_THEME_ID,
    label: `Omarchy · ${name}`,
    scheme,
    ansi,
    // Omarchy's own terminal pair (`cursor`/`bright_foreground`, `selection`),
    // as its T3 Code template publishes them.
    terminal: { cursor: c.cursor, selectionForeground: c.selection_foreground, selectionBackground: c.selection_background },
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
      sidebarBorder, sidebarRing: c.accent, brandAccent: c.accent,
    },
  };
}
