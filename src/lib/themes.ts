import type { ThemeColors } from "@/tauri/types";
import { syncWindowBackground } from "@/lib/window-background";

export const THEME_FILE_VERSION = 1 as const;
export const THEME_BOOT_STORAGE_KEY = "codemux:appearance-theme:v1";

export const THEME_ROLES = [
  "background",
  "foreground",
  "card",
  "cardForeground",
  "popover",
  "popoverForeground",
  "primary",
  "primaryForeground",
  "secondary",
  "secondaryForeground",
  "muted",
  "mutedForeground",
  "accent",
  "accentForeground",
  "border",
  "input",
  "ring",
  "sidebar",
  "sidebarForeground",
  "sidebarPrimary",
  "sidebarPrimaryForeground",
  "sidebarAccent",
  "sidebarAccentForeground",
  "sidebarBorder",
  "sidebarRing",
  "brandAccent",
] as const;

export type ThemeRole = (typeof THEME_ROLES)[number];
export type ThemeRoleMap = Readonly<Record<ThemeRole, string>>;

export const ANSI_SLOTS = [
  "black",
  "red",
  "green",
  "yellow",
  "blue",
  "magenta",
  "cyan",
  "white",
  "brightBlack",
  "brightRed",
  "brightGreen",
  "brightYellow",
  "brightBlue",
  "brightMagenta",
  "brightCyan",
  "brightWhite",
] as const;

export type AnsiSlot = (typeof ANSI_SLOTS)[number];
export type AnsiPalette = Readonly<Record<AnsiSlot, string>>;

/**
 * Whether a palette paints dark ink on a light canvas or the reverse.
 *
 * This is derived from the background, never declared: a file that says
 * `"dark"` over a white canvas is simply wrong about itself, and the
 * palette is the thing the user actually sees.
 */
export type ThemeScheme = "light" | "dark";

/**
 * The luminance above which a canvas is a *light* canvas.
 *
 * 0.28 is the same threshold the generator used when it refused light
 * backgrounds outright, kept so no existing dark theme changes classification.
 */
const LIGHT_SCHEME_LUMINANCE = 0.28;

export function schemeForBackground(color: string): ThemeScheme {
  return relativeLuminance(color) >= LIGHT_SCHEME_LUMINANCE ? "light" : "dark";
}

export interface ThemeDefinition {
  id: string;
  label: string;
  scheme: ThemeScheme;
  roles: ThemeRoleMap;
  ansi: AnsiPalette;
  radius?: string;
  managed?: boolean;
  seeds?: { background: string; accent: string };
  source?: "generated" | "shadcn" | "vscode" | "json";
}

export interface ThemeFile {
  version: typeof THEME_FILE_VERSION;
  id: string;
  label: string;
  scheme: ThemeScheme;
  roles: Partial<Record<ThemeRole, string>>;
  ansi?: Partial<Record<AnsiSlot, string>>;
  radius?: string;
  managed?: boolean;
  seeds?: { background: string; accent: string };
  source?: ThemeDefinition["source"];
}

const ROLE_VARIABLES: Readonly<Record<ThemeRole, string>> = {
  background: "--cm-theme-background",
  foreground: "--cm-theme-foreground",
  card: "--cm-theme-card",
  cardForeground: "--cm-theme-card-foreground",
  popover: "--cm-theme-popover",
  popoverForeground: "--cm-theme-popover-foreground",
  primary: "--cm-theme-primary",
  primaryForeground: "--cm-theme-primary-foreground",
  secondary: "--cm-theme-secondary",
  secondaryForeground: "--cm-theme-secondary-foreground",
  muted: "--cm-theme-muted",
  mutedForeground: "--cm-theme-muted-foreground",
  accent: "--cm-theme-accent",
  accentForeground: "--cm-theme-accent-foreground",
  border: "--cm-theme-border",
  input: "--cm-theme-input",
  ring: "--cm-theme-ring",
  sidebar: "--cm-theme-sidebar",
  sidebarForeground: "--cm-theme-sidebar-foreground",
  sidebarPrimary: "--cm-theme-sidebar-primary",
  sidebarPrimaryForeground: "--cm-theme-sidebar-primary-foreground",
  sidebarAccent: "--cm-theme-sidebar-accent",
  sidebarAccentForeground: "--cm-theme-sidebar-accent-foreground",
  sidebarBorder: "--cm-theme-sidebar-border",
  sidebarRing: "--cm-theme-sidebar-ring",
  brandAccent: "--cm-theme-brand-accent",
};

const ANSI_VARIABLES: Readonly<Record<AnsiSlot, string>> = Object.fromEntries(
  ANSI_SLOTS.map((slot) => [slot, `--cm-ansi-${slot.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}`]),
) as Record<AnsiSlot, string>;

type Rgb = { r: number; g: number; b: number };
type Oklch = { L: number; C: number; h: number };

const DEFAULT_ANSI: AnsiPalette = {
  black: "#151110",
  red: "#dc6b6b",
  green: "#7ec699",
  yellow: "#e5c07b",
  blue: "#61afef",
  magenta: "#c678dd",
  cyan: "#56b6c2",
  white: "#eae8e6",
  brightBlack: "#5c5856",
  brightRed: "#e88888",
  brightGreen: "#98d1a8",
  brightYellow: "#ecd08f",
  brightBlue: "#7ec0f5",
  brightMagenta: "#d494e6",
  brightCyan: "#73c7d3",
  brightWhite: "#ffffff",
};

/**
 * ANSI for a light terminal.
 *
 * The default foreground/background swap when the scheme does, but the named
 * slots must not: `black` still has to be dark and `white` still has to be
 * light, or every program that prints `\e[30m` becomes unreadable. Only the
 * *lightness* of the coloured slots moves, down to roughly L 0.55 so each one
 * clears 4:1 on a white canvas.
 */
const LIGHT_ANSI: AnsiPalette = {
  black: "#1c1917",
  red: "#c23f3f",
  green: "#2f8f4e",
  yellow: "#a86f00",
  blue: "#2f6fd0",
  magenta: "#9a4fbd",
  cyan: "#1a7f8e",
  white: "#d6d3d1",
  brightBlack: "#78716c",
  // "Bright" on a light canvas means *more present*, which is darker and a
  // touch more saturated — a lighter variant would simply disappear.
  brightRed: "#a83232",
  brightGreen: "#25753f",
  brightYellow: "#8a5a00",
  brightBlue: "#2559ab",
  brightMagenta: "#7f3f9c",
  brightCyan: "#136774",
  brightWhite: "#ffffff",
};

const DEFAULT_ROLES: ThemeRoleMap = {
  background: "oklch(0.147 0.004 49.25)",
  foreground: "oklch(0.985 0.001 106.423)",
  card: "oklch(0.216 0.006 56.043)",
  cardForeground: "oklch(0.985 0.001 106.423)",
  popover: "oklch(0.216 0.006 56.043)",
  popoverForeground: "oklch(0.985 0.001 106.423)",
  primary: "oklch(0.923 0.003 48.717)",
  primaryForeground: "oklch(0.216 0.006 56.043)",
  secondary: "oklch(0.268 0.007 34.298)",
  secondaryForeground: "oklch(0.985 0.001 106.423)",
  muted: "oklch(0.268 0.007 34.298)",
  mutedForeground: "oklch(0.709 0.01 56.259)",
  accent: "oklch(0.268 0.007 34.298)",
  accentForeground: "oklch(0.985 0.001 106.423)",
  border: "oklch(1 0 0 / 10%)",
  input: "oklch(1 0 0 / 15%)",
  ring: "oklch(0.553 0.013 58.071)",
  sidebar: "oklch(0.13 0.004 49)",
  sidebarForeground: "oklch(0.985 0.001 106.423)",
  sidebarPrimary: "oklch(0.705 0.152 47)",
  sidebarPrimaryForeground: "oklch(0.205 0.006 40)",
  sidebarAccent: "oklch(0.268 0.007 34.298)",
  sidebarAccentForeground: "oklch(0.985 0.001 106.423)",
  sidebarBorder: "oklch(1 0 0 / 10%)",
  sidebarRing: "oklch(0.553 0.013 58.071)",
  brandAccent: "oklch(0.705 0.152 47)",
};

function roles(overrides: Partial<ThemeRoleMap>): ThemeRoleMap {
  return { ...DEFAULT_ROLES, ...overrides };
}

function ansi(overrides: Partial<AnsiPalette>): AnsiPalette {
  return { ...DEFAULT_ANSI, ...overrides };
}

export const BUILT_IN_THEMES: readonly ThemeDefinition[] = [
  {
    id: "default",
    label: "Graphite",
    scheme: "dark",
    roles: DEFAULT_ROLES,
    ansi: DEFAULT_ANSI,
  },
  {
    /**
     * Graphite's light twin: the shadcn stone light ramp Codemux already
     * shipped (dead in `:root` because the runtime bridge always won) plus
     * Graphite's ember brand.
     *
     * The brand accent is ember at L 0.58 rather than Graphite's 0.705 —
     * `oklch(0.705 0.152 47)` only reaches 2.76:1 on white, and `brandAccent`
     * has to work as inline code and as a status dot directly on the canvas.
     * `oklch(0.58 0.152 47)` is the same hue and chroma at 4.55:1.
     */
    id: "graphite-light",
    label: "Graphite Light",
    scheme: "light",
    roles: roles({
      background: "oklch(1 0 0)",
      foreground: "oklch(0.147 0.004 49.25)",
      card: "oklch(1 0 0)",
      cardForeground: "oklch(0.147 0.004 49.25)",
      popover: "oklch(1 0 0)",
      popoverForeground: "oklch(0.147 0.004 49.25)",
      primary: "oklch(0.216 0.006 56.043)",
      primaryForeground: "oklch(0.985 0.001 106.423)",
      secondary: "oklch(0.97 0.001 106.424)",
      secondaryForeground: "oklch(0.216 0.006 56.043)",
      muted: "oklch(0.97 0.001 106.424)",
      mutedForeground: "oklch(0.553 0.013 58.071)",
      accent: "oklch(0.97 0.001 106.424)",
      accentForeground: "oklch(0.216 0.006 56.043)",
      border: "oklch(0.923 0.003 48.717)",
      input: "oklch(0.923 0.003 48.717)",
      ring: "oklch(0.709 0.01 56.259)",
      // The rail still recedes, so on a white canvas it is the *darker*
      // surface — the same inversion the dark themes make in the other
      // direction.
      sidebar: "oklch(0.972 0.001 106.4)",
      sidebarForeground: "oklch(0.147 0.004 49.25)",
      sidebarPrimary: "oklch(0.58 0.152 47)",
      sidebarPrimaryForeground: "oklch(0.985 0.001 106.423)",
      sidebarAccent: "oklch(0.955 0.002 106.4)",
      sidebarAccentForeground: "oklch(0.216 0.006 56.043)",
      sidebarBorder: "oklch(0.923 0.003 48.717)",
      sidebarRing: "oklch(0.709 0.01 56.259)",
      brandAccent: "oklch(0.58 0.152 47)",
    }),
    ansi: LIGHT_ANSI,
  },
  {
    id: "warm",
    label: "Warm Stone",
    scheme: "dark",
    roles: roles({
      background: "oklch(0.178 0.006 40)",
      foreground: "oklch(0.935 0.004 67)",
      card: "oklch(0.238 0.006 58)",
      cardForeground: "oklch(0.935 0.004 67)",
      popover: "oklch(0.238 0.006 58)",
      popoverForeground: "oklch(0.935 0.004 67)",
      primary: "oklch(0.925 0.004 70)",
      primaryForeground: "oklch(0.205 0.006 40)",
      secondary: "oklch(0.272 0.006 55)",
      secondaryForeground: "oklch(0.935 0.004 67)",
      muted: "oklch(0.272 0.006 55)",
      mutedForeground: "oklch(0.725 0.006 56)",
      accent: "oklch(0.272 0.006 55)",
      accentForeground: "oklch(0.935 0.004 67)",
      border: "oklch(0.315 0.006 55)",
      input: "oklch(0.315 0.006 55)",
      ring: "oklch(0.55 0.009 56)",
      sidebar: "oklch(0.165 0.006 50)",
      sidebarForeground: "oklch(0.935 0.004 67)",
      sidebarAccent: "oklch(0.255 0.006 58)",
      sidebarAccentForeground: "oklch(0.935 0.004 67)",
      sidebarBorder: "oklch(0.272 0.006 55)",
      sidebarRing: "oklch(0.55 0.009 56)",
    }),
    ansi: ansi({ black: "#151110", white: "#eae5e1" }),
  },
  {
    id: "ember",
    label: "Ember",
    scheme: "dark",
    roles: roles({
      background: "#151110",
      foreground: "#eae8e6",
      card: "#201e1c",
      cardForeground: "#eae8e6",
      popover: "#201e1c",
      popoverForeground: "#eae8e6",
      primary: "#e07850",
      primaryForeground: "#151110",
      secondary: "#2a2827",
      secondaryForeground: "#eae8e6",
      muted: "#2a2827",
      mutedForeground: "#a8a5a3",
      accent: "#2a2827",
      accentForeground: "#eae8e6",
      border: "#383330",
      input: "#383330",
      ring: "#5d5753",
      sidebar: "#1a1716",
      sidebarForeground: "#eae8e6",
      sidebarPrimary: "#e07850",
      sidebarPrimaryForeground: "#151110",
      sidebarAccent: "#252220",
      sidebarAccentForeground: "#eae8e6",
      sidebarBorder: "#2a2827",
      sidebarRing: "#5d5753",
      brandAccent: "#e07850",
    }),
    ansi: DEFAULT_ANSI,
  },
  {
    id: "ocean",
    label: "Abyss",
    scheme: "dark",
    roles: roles({
      background: "#07151b",
      foreground: "#d9edf0",
      card: "#0c2028",
      cardForeground: "#d9edf0",
      popover: "#102933",
      popoverForeground: "#d9edf0",
      primary: "#78d6df",
      primaryForeground: "#061418",
      secondary: "#12313b",
      secondaryForeground: "#d9edf0",
      muted: "#102a33",
      mutedForeground: "#89aeb4",
      accent: "#153945",
      accentForeground: "#d9edf0",
      border: "#20414a",
      input: "#28505a",
      ring: "#4f7d84",
      sidebar: "#051116",
      sidebarForeground: "#d9edf0",
      sidebarPrimary: "#78d6df",
      sidebarPrimaryForeground: "#061418",
      sidebarAccent: "#102a33",
      sidebarAccentForeground: "#d9edf0",
      sidebarBorder: "#18353e",
      sidebarRing: "#4f7d84",
      brandAccent: "#78d6df",
    }),
    ansi: ansi({
      black: "#061217", red: "#f07178", green: "#9ece6a", yellow: "#e0af68",
      blue: "#69c7d3", magenta: "#bb9af7", cyan: "#7dcfff", white: "#c7dadd",
      brightBlack: "#48646a", brightBlue: "#8bdce5", brightCyan: "#a5ecf2",
    }),
  },
  {
    id: "iris",
    label: "Iris",
    scheme: "dark",
    roles: roles({
      background: "#15111d",
      foreground: "#eee9f5",
      card: "#211a2c",
      cardForeground: "#eee9f5",
      popover: "#261e32",
      popoverForeground: "#eee9f5",
      primary: "#c9a7ff",
      primaryForeground: "#1b1325",
      secondary: "#302440",
      secondaryForeground: "#eee9f5",
      muted: "#2a2136",
      mutedForeground: "#aa9bbd",
      accent: "#352747",
      accentForeground: "#f2ebfa",
      border: "#413451",
      input: "#4a3a5d",
      ring: "#756486",
      sidebar: "#100c17",
      sidebarForeground: "#eee9f5",
      sidebarPrimary: "#c9a7ff",
      sidebarPrimaryForeground: "#1b1325",
      sidebarAccent: "#281e35",
      sidebarAccentForeground: "#eee9f5",
      sidebarBorder: "#352945",
      sidebarRing: "#756486",
      brandAccent: "#c9a7ff",
    }),
    ansi: ansi({
      black: "#120e19", red: "#ef7a8a", green: "#8bd49c", yellow: "#e6c27a",
      blue: "#87a9ff", magenta: "#c9a7ff", cyan: "#75d1cf", white: "#ded5e8",
      brightBlack: "#64566f", brightMagenta: "#dfc8ff", brightBlue: "#aac1ff",
    }),
  },
] as const;

function parseHex(value: string): (Rgb & { a: number }) | null {
  let raw = value.trim().replace(/^#/, "");
  if (raw.length === 3 || raw.length === 4) raw = raw.split("").map((c) => c + c).join("");
  if (!/^[0-9a-f]{6}([0-9a-f]{2})?$/i.test(raw)) return null;
  return {
    r: Number.parseInt(raw.slice(0, 2), 16),
    g: Number.parseInt(raw.slice(2, 4), 16),
    b: Number.parseInt(raw.slice(4, 6), 16),
    a: raw.length === 8 ? Number.parseInt(raw.slice(6, 8), 16) / 255 : 1,
  };
}

function rgbToHex({ r, g, b }: Rgb): string {
  const channel = (value: number) => Math.round(Math.max(0, Math.min(255, value))).toString(16).padStart(2, "0");
  return `#${channel(r)}${channel(g)}${channel(b)}`;
}

function linearToSrgb(value: number): number {
  const c = value <= 0.0031308 ? value * 12.92 : 1.055 * value ** (1 / 2.4) - 0.055;
  return Math.max(0, Math.min(255, c * 255));
}

function srgbToLinear(value: number): number {
  const c = value / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

function oklchToRgb(color: Oklch): Rgb {
  let chroma = color.C;
  for (let attempt = 0; attempt < 16; attempt += 1) {
    const h = (color.h * Math.PI) / 180;
    const a = chroma * Math.cos(h);
    const b = chroma * Math.sin(h);
    const l = (color.L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
    const m = (color.L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
    const s = (color.L - 0.0894841775 * a - 1.291485548 * b) ** 3;
    const linear = {
      r: 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
      g: -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
      b: -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
    };
    if (Object.values(linear).every((c) => c >= -0.0001 && c <= 1.0001)) {
      return { r: linearToSrgb(linear.r), g: linearToSrgb(linear.g), b: linearToSrgb(linear.b) };
    }
    chroma *= 0.82;
  }
  return oklchToRgb({ ...color, C: 0 });
}

function rgbToOklch(color: Rgb): Oklch {
  const r = srgbToLinear(color.r);
  const g = srgbToLinear(color.g);
  const b = srgbToLinear(color.b);
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const L = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
  const a = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const bb = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return { L, C: Math.hypot(a, bb), h: (Math.atan2(bb, a) * 180) / Math.PI };
}

function parseOklch(value: string): (Rgb & { a: number }) | null {
  const match = /^oklch\(\s*([\d.]+)(%)?\s+([\d.]+)\s+(-?[\d.]+)(?:\s*\/\s*([\d.]+)(%)?)?\s*\)$/i.exec(value.trim());
  if (!match) return null;
  const rawLightness = Number(match[1]);
  const L = match[2] === "%" ? rawLightness / 100 : rawLightness;
  const C = Number(match[3]);
  const h = Number(match[4]);
  if (![L, C, h].every(Number.isFinite)) return null;
  const alphaValue = match[5] === undefined ? 1 : Number(match[5]);
  const a = match[6] === "%" ? alphaValue / 100 : alphaValue;
  if (!Number.isFinite(a)) return null;
  return { ...oklchToRgb({ L, C, h }), a: Math.max(0, Math.min(1, a)) };
}

export function normalizeColor(value: string, backdrop = "#000000"): string | null {
  const parsed = parseHex(value) ?? parseOklch(value);
  if (!parsed) return null;
  if (parsed.a >= 1) return rgbToHex(parsed);
  const base = parseHex(backdrop) ?? { r: 0, g: 0, b: 0, a: 1 };
  return rgbToHex({
    r: Math.round(parsed.r) * parsed.a + base.r * (1 - parsed.a),
    g: Math.round(parsed.g) * parsed.a + base.g * (1 - parsed.a),
    b: Math.round(parsed.b) * parsed.a + base.b * (1 - parsed.a),
  });
}

export function relativeLuminance(value: string): number {
  const color = parseHex(normalizeColor(value) ?? "#000000")!;
  return 0.2126 * srgbToLinear(color.r) + 0.7152 * srgbToLinear(color.g) + 0.0722 * srgbToLinear(color.b);
}

export function contrastRatio(first: string, second: string): number {
  const a = relativeLuminance(first);
  const b = relativeLuminance(second);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

function solveLightness(base: Oklch, against: string, minimum: number, direction: "lighter" | "darker"): string {
  let low = direction === "lighter" ? base.L : 0;
  let high = direction === "lighter" ? 1 : base.L;
  let best = base.L;
  for (let index = 0; index < 20; index += 1) {
    const mid = (low + high) / 2;
    const candidate = rgbToHex(oklchToRgb({ ...base, L: mid }));
    if (contrastRatio(candidate, against) >= minimum) {
      best = mid;
      if (direction === "lighter") high = mid;
      else low = mid;
    } else if (direction === "lighter") low = mid;
    else high = mid;
  }
  return rgbToHex(oklchToRgb({ ...base, L: best }));
}

function readableOn(background: string, tintedHue?: number): string {
  const light = tintedHue === undefined ? "#f8f7f5" : rgbToHex(oklchToRgb({ L: 0.96, C: 0.018, h: tintedHue }));
  const dark = tintedHue === undefined ? "#171411" : rgbToHex(oklchToRgb({ L: 0.2, C: 0.018, h: tintedHue }));
  return contrastRatio(light, background) >= contrastRatio(dark, background) ? light : dark;
}

/**
 * How far a light theme's surfaces travel compared with a dark one's.
 *
 * The deltas below were tuned against a near-black canvas, where there is a
 * whole unit of headroom above. A white canvas has the same headroom below,
 * but the eye reads a drop from white far more strongly than the same rise
 * from black, so `input` at a literal `1.0 - 0.2` lands on a mid grey that
 * looks like a disabled control. Scaling the light side keeps the *ordering*
 * of the ramp identical while landing `border` and `input` near the values
 * the shadcn stone light preset uses.
 */
const LIGHT_SURFACE_SCALE = 0.6;

function surface(canvas: Oklch, hue: number, chroma: number, delta: number, scheme: ThemeScheme): string {
  const L =
    scheme === "dark"
      ? Math.min(0.96, canvas.L + delta)
      : Math.max(0.04, canvas.L - delta * LIGHT_SURFACE_SCALE);
  return rgbToHex(oklchToRgb({ L, C: chroma, h: hue }));
}

function lighten(value: string, amount: number): string {
  const color = rgbToOklch(parseHex(normalizeColor(value) ?? "#000000")!);
  return rgbToHex(oklchToRgb({ ...color, L: Math.min(0.94, color.L + amount) }));
}

/** `lighten`'s mirror, with a floor so a "bright" slot never bottoms out at black. */
function darken(value: string, amount: number): string {
  const color = rgbToOklch(parseHex(normalizeColor(value) ?? "#ffffff")!);
  return rgbToHex(oklchToRgb({ ...color, L: Math.max(0.12, color.L - amount) }));
}

function deriveAnsi(background: string, foreground: string, accent: string, scheme: ThemeScheme): AnsiPalette {
  const accentOklch = rgbToOklch(parseHex(accent)!);
  const chroma = Math.max(0.08, Math.min(0.17, accentOklch.C));
  const hueColor = (hue: number, lightness: number) => rgbToHex(oklchToRgb({ L: lightness, C: chroma, h: (hue + 360) % 360 }));
  if (scheme === "light") {
    /**
     * Only the terminal's *default* pair follows the scheme. The sixteen
     * named slots do not: `black` stays dark and `white` stays light on a
     * white canvas too, otherwise anything that hardcodes `\e[30m` (git,
     * most TUIs) paints white-on-white. So `black` takes the theme's dark
     * ink rather than the canvas, and `white`/`brightWhite` stay at the
     * light end where their names promise they are.
     */
    const base = {
      black: foreground,
      red: "#c23f3f",
      green: "#2f8f4e",
      yellow: "#a86f00",
      blue: hueColor(accentOklch.h, 0.5),
      magenta: hueColor(accentOklch.h + 55, 0.5),
      cyan: hueColor(accentOklch.h - 55, 0.5),
      white: darken(background, 0.12),
    };
    return {
      ...base,
      brightBlack: darken(background, 0.45),
      // Brights go *down* in lightness here: on a white canvas the more
      // prominent version of a colour is the darker one.
      brightRed: darken(base.red, 0.08),
      brightGreen: darken(base.green, 0.06),
      brightYellow: darken(base.yellow, 0.05),
      brightBlue: darken(base.blue, 0.08),
      brightMagenta: darken(base.magenta, 0.08),
      brightCyan: darken(base.cyan, 0.08),
      brightWhite: "#ffffff",
    };
  }
  const base = {
    black: background,
    red: "#dc6b6b",
    green: "#7ec699",
    yellow: "#e5c07b",
    blue: hueColor(accentOklch.h, 0.7),
    magenta: hueColor(accentOklch.h + 55, 0.7),
    cyan: hueColor(accentOklch.h - 55, 0.7),
    white: foreground,
  };
  return {
    ...base,
    brightBlack: lighten(background, 0.25),
    brightRed: lighten(base.red, 0.08),
    brightGreen: lighten(base.green, 0.06),
    brightYellow: lighten(base.yellow, 0.05),
    brightBlue: lighten(base.blue, 0.08),
    brightMagenta: lighten(base.magenta, 0.08),
    brightCyan: lighten(base.cyan, 0.08),
    brightWhite: "#ffffff",
  };
}

export function createGeneratedTheme(
  label: string,
  backgroundValue: string,
  accentValue: string,
  id = themeIdFromLabel(label),
): ThemeDefinition {
  const background = normalizeColor(backgroundValue);
  const accent = normalizeColor(accentValue, background ?? "#000000");
  if (!background || !accent) throw new Error("Use a valid hex or OKLCH background and accent.");

  /**
   * The canvas decides the scheme, and the scheme decides which way every
   * derivation points: text is solved *away* from the background, surfaces
   * step away from it, and the ANSI ramp is picked for the canvas it will be
   * printed on.
   */
  const scheme = schemeForBackground(background);
  const canvas = rgbToOklch(parseHex(background)!);
  const accentColor = rgbToOklch(parseHex(accent)!);

  /**
   * Surfaces and text stay in the **background's** hue family; only genuinely
   * accent-colored roles take the accent's.
   *
   * These two inputs are "the room" and "the highlight", not one input and a
   * multiplier. Deriving every surface from the accent hue meant a blue-black
   * canvas with a warm accent produced a *brown* app — the background you
   * picked survived only in the one role literally named `background`.
   *
   * Chroma is likewise bounded by the canvas's own, so a neutral grey
   * background yields a neutral grey ramp instead of inventing a tint the
   * user never asked for.
   */
  const hue = canvas.h;
  const tint = Math.min(0.03, canvas.C);
  const surfaceChroma = (scale: number, ceiling: number) =>
    Math.min(ceiling, canvas.C * scale);
  /** Text is solved towards the far end of the ramp from the canvas. */
  const ink: "lighter" | "darker" = scheme === "dark" ? "lighter" : "darker";
  const step = (chroma: number, delta: number) => surface(canvas, hue, chroma, delta, scheme);

  const foreground = solveLightness(
    { L: scheme === "dark" ? 0.94 : 0.22, C: Math.min(0.02, canvas.C * 0.6), h: hue },
    background,
    7,
    ink,
  );
  // The seed is a *bound*, not the answer: the solver returns the least
  // extreme lightness that still clears the target, so 0.55 lets muted text on
  // a white canvas settle near the stone preset's own 4.8:1 rather than being
  // dragged to body-text weight.
  const mutedForeground = solveLightness({ L: scheme === "dark" ? 0.65 : 0.55, C: tint, h: hue }, background, 4.7, ink);
  // This one sits *on* the accent, so it is the accent's hue that has to
  // carry it.
  const accentForeground = readableOn(accent, accentColor.h);
  const sidebar = step(tint * 1.2, 0.025);
  const card = step(tint, 0.055);
  const secondary = step(surfaceChroma(1.3, 0.06), 0.1);
  const border = step(surfaceChroma(1.1, 0.05), 0.16);
  const input = step(surfaceChroma(1.2, 0.055), 0.2);
  const themeRoles = roles({
    background,
    foreground,
    card,
    cardForeground: foreground,
    popover: step(tint, 0.075),
    popoverForeground: foreground,
    primary: accent,
    primaryForeground: accentForeground,
    secondary,
    secondaryForeground: solveLightness(rgbToOklch(parseHex(foreground)!), secondary, 4.6, ink),
    muted: step(tint, 0.075),
    mutedForeground,
    accent: step(surfaceChroma(1.4, 0.07), 0.12),
    accentForeground: foreground,
    border,
    input,
    ring: step(tint, 0.3),
    sidebar,
    sidebarForeground: solveLightness(rgbToOklch(parseHex(foreground)!), sidebar, 4.6, ink),
    sidebarPrimary: accent,
    sidebarPrimaryForeground: accentForeground,
    sidebarAccent: step(surfaceChroma(1.25, 0.055), 0.09),
    sidebarAccentForeground: foreground,
    sidebarBorder: step(surfaceChroma(1.0, 0.045), 0.13),
    sidebarRing: step(tint, 0.3),
    brandAccent: accent,
  });
  return {
    id,
    label: label.trim().slice(0, 48) || "Custom theme",
    scheme,
    roles: themeRoles,
    ansi: deriveAnsi(background, foreground, accent, scheme),
    managed: true,
    seeds: { background, accent },
    source: "generated",
  };
}

export function themeIdFromLabel(label: string): string {
  const base = label.trim().toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 40);
  return `custom-${base || "theme"}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function validRadius(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  return /^\d*\.?\d+(?:px|rem|em)$/.test(value.trim()) ? value.trim() : undefined;
}

function completeThemeFile(file: ThemeFile): ThemeDefinition {
  const seedBackground = file.seeds?.background ?? file.roles.background ?? "#151110";
  const seedAccent = file.seeds?.accent ?? file.roles.brandAccent ?? file.roles.primary ?? "#e07850";
  const generated = createGeneratedTheme(file.label, seedBackground, seedAccent, file.id);
  const themeRoles = { ...generated.roles };
  for (const role of THEME_ROLES) {
    const value = file.roles[role];
    if (typeof value === "string") {
      const normalized = normalizeColor(value, normalizeColor(themeRoles.background) ?? "#000000");
      if (normalized) themeRoles[role] = normalized;
    }
  }
  const themeAnsi = { ...generated.ansi };
  for (const slot of ANSI_SLOTS) {
    const normalized = typeof file.ansi?.[slot] === "string" ? normalizeColor(file.ansi[slot]!) : null;
    if (normalized) themeAnsi[slot] = normalized;
  }
  return {
    ...generated,
    id: /^custom-[a-z0-9-]{1,48}$/.test(file.id) ? file.id : themeIdFromLabel(file.label),
    label: file.label.trim().slice(0, 48) || "Custom theme",
    // `file.scheme` is advisory. A file that declares "dark" over a white
    // canvas is wrong about itself, and the palette is what the user sees, so
    // the background always wins.
    scheme: schemeForBackground(themeRoles.background),
    roles: themeRoles,
    ansi: themeAnsi,
    radius: validRadius(file.radius),
    managed: file.managed === true,
    source: file.source ?? "json",
  };
}

export function parseCustomTheme(value: unknown): ThemeDefinition | null {
  if (!isRecord(value) || typeof value.label !== "string" || !isRecord(value.roles)) return null;
  try {
    return completeThemeFile({
      version: THEME_FILE_VERSION,
      id: typeof value.id === "string" ? value.id : themeIdFromLabel(value.label),
      label: value.label,
      scheme: value.scheme === "light" || value.scheme === "dark" ? value.scheme : "dark",
      roles: value.roles as Partial<Record<ThemeRole, string>>,
      ansi: isRecord(value.ansi) ? value.ansi as Partial<Record<AnsiSlot, string>> : undefined,
      radius: validRadius(value.radius),
      managed: value.managed === true,
      seeds: isRecord(value.seeds) && typeof value.seeds.background === "string" && typeof value.seeds.accent === "string"
        ? { background: value.seeds.background, accent: value.seeds.accent }
        : undefined,
      source: value.source === "generated" || value.source === "shadcn" || value.source === "vscode" || value.source === "json"
        ? value.source
        : "json",
    });
  } catch {
    return null;
  }
}

export function parseCustomThemes(value: unknown): ThemeDefinition[] {
  if (!Array.isArray(value)) return [];
  const result: ThemeDefinition[] = [];
  for (const entry of value) {
    const theme = parseCustomTheme(entry);
    if (theme && !result.some((existing) => existing.id === theme.id)) result.push(theme);
  }
  return result;
}

export function serializeTheme(theme: ThemeDefinition): string {
  const file: ThemeFile = {
    version: THEME_FILE_VERSION,
    id: theme.id,
    label: theme.label,
    scheme: theme.scheme,
    roles: theme.roles,
    ansi: theme.ansi,
    radius: theme.radius,
    managed: theme.managed,
    seeds: theme.seeds,
    source: theme.source,
  };
  return `${JSON.stringify(file, null, 2)}\n`;
}

function stripJsonComments(text: string): string {
  return text
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/(^|[^:\"'\\])\/\/[^\n\r]*/g, "$1")
    .replace(/,(\s*[}\]])/g, "$1");
}

/**
 * What an import actually got from its source.
 *
 * The studio states the format and the count instead of making the user find
 * out by applying the theme, and lists the roles Codemux had to solve for —
 * so `derived` is the honest part: those colors are ours, not the author's.
 */
export type ThemeImportSource = "shadcn" | "vscode" | "codemux";

export interface ThemeImportResult {
  theme: ThemeDefinition;
  source: ThemeImportSource;
  /** Roles the pasted source specified outright. */
  mapped: ThemeRole[];
  /** Roles the source omitted, filled in by the generator. */
  derived: ThemeRole[];
}

function splitRoles(mapped: Iterable<ThemeRole>): Pick<ThemeImportResult, "mapped" | "derived"> {
  const present = new Set(mapped);
  return {
    mapped: THEME_ROLES.filter((role) => present.has(role)),
    derived: THEME_ROLES.filter((role) => !present.has(role)),
  };
}

function importShadcn(text: string, label: string): { theme: ThemeDefinition; mapped: Set<ThemeRole> } {
  const declarations = new Map<string, string>();
  for (const match of text.matchAll(/--([a-z0-9-]+)\s*:\s*([^;}{]+)\s*;/gi)) declarations.set(match[1]!, match[2]!.trim());
  const background = declarations.get("background");
  const accent = declarations.get("sidebar-primary") ?? declarations.get("primary") ?? declarations.get("ring");
  if (!background || !accent) throw new Error("Paste a shadcn variable block containing --background and --primary.");
  const generated = createGeneratedTheme(label, background, accent);
  const mapping: Partial<Record<string, ThemeRole>> = {
    background: "background", foreground: "foreground", card: "card", "card-foreground": "cardForeground",
    popover: "popover", "popover-foreground": "popoverForeground", primary: "primary",
    "primary-foreground": "primaryForeground", secondary: "secondary", "secondary-foreground": "secondaryForeground",
    muted: "muted", "muted-foreground": "mutedForeground", accent: "accent", "accent-foreground": "accentForeground",
    border: "border", input: "input", ring: "ring", sidebar: "sidebar", "sidebar-foreground": "sidebarForeground",
    "sidebar-primary": "sidebarPrimary", "sidebar-primary-foreground": "sidebarPrimaryForeground",
    "sidebar-accent": "sidebarAccent", "sidebar-accent-foreground": "sidebarAccentForeground",
    "sidebar-border": "sidebarBorder", "sidebar-ring": "sidebarRing",
  };
  const imported = { ...generated.roles };
  const mapped = new Set<ThemeRole>();
  for (const [token, role] of Object.entries(mapping)) {
    const raw = declarations.get(token);
    const normalized = raw ? normalizeColor(raw, normalizeColor(imported.background) ?? "#000000") : null;
    if (role && normalized) {
      imported[role] = normalized;
      mapped.add(role);
    }
  }
  imported.brandAccent = imported.sidebarPrimary;
  if (mapped.has("sidebarPrimary")) mapped.add("brandAccent");
  return {
    theme: {
      ...generated,
      scheme: schemeForBackground(imported.background),
      roles: imported,
      radius: validRadius(declarations.get("radius")),
      source: "shadcn",
      managed: false,
    },
    mapped,
  };
}

function importVsCode(
  value: Record<string, unknown>,
  fallbackLabel: string,
): { theme: ThemeDefinition; mapped: Set<ThemeRole> } {
  if (!isRecord(value.colors)) throw new Error("That VS Code theme has no colors map.");
  const colors = value.colors;

  /**
   * The manifest's own `type` — `"light" | "dark" | "hc" | "hcLight"`.
   *
   * It does **not** decide the scheme: `editor.background` does, because
   * that is the canvas the user will actually look at and a file that
   * mislabels itself is simply wrong about itself. What the declaration is
   * good for is the one moment before the canvas is known: a translucent
   * colour has to be composited over *something*, and on a light theme that
   * something is white. Getting this wrong flattened `#ffffffcc` to a mid
   * grey and made a white theme import as a dark one.
   */
  const declaredType = typeof value.type === "string" ? value.type.toLowerCase() : "";
  const declaredLight = declaredType === "light" || declaredType === "hclight";
  let importBackdrop = declaredLight ? "#ffffff" : "#000000";
  const pick = (...keys: string[]) => {
    for (const key of keys) {
      if (typeof colors[key] === "string") {
        const normalized = normalizeColor(colors[key] as string, importBackdrop);
        if (normalized) return normalized;
      }
    }
    return null;
  };
  const background = pick("editor.background", "editorPane.background");
  if (!background) throw new Error('That VS Code theme has no "editor.background" color.');
  importBackdrop = background;
  /** The canvas decides, and every "away from the background" step follows it. */
  const scheme = schemeForBackground(background);
  const away: "lighter" | "darker" = scheme === "dark" ? "lighter" : "darker";

  /**
   * The theme's identity color.
   *
   * Ordered by how deliberately each token is chosen by a theme author.
   * `focusBorder` is **last**: it is very often a translucent grey — Tokyo
   * Night's is `#545c7e33` — which, flattened over a dark canvas, produced a
   * mud-colored brand accent for a theme everyone recognises as blue.
   *
   * Codemux's `brandAccent` has to work as inline code and as a status dot on
   * the canvas, so a candidate that can't clear 3:1 against the background
   * isn't usable no matter how deliberate it was; the terminal's blue/cyan
   * are the reliable bright fallbacks, and failing those the pick is pushed
   * away from the canvas — lighter on a dark theme, darker on a light one —
   * until it's legible.
   */
  const accentCandidates = [
    pick("activityBarBadge.background"),
    pick("button.background"),
    pick("progressBar.background"),
    pick("textLink.foreground"),
    pick("terminal.ansiBlue"),
    pick("terminal.ansiCyan"),
    pick("focusBorder"),
  ].filter((value): value is string => value !== null);
  const legible = accentCandidates.find((value) => contrastRatio(value, background) >= 3);
  const accent =
    legible ??
    solveLightness(rgbToOklch(parseHex(accentCandidates[0] ?? "#e07850")!), background, 3, away);
  const label = typeof value.name === "string" ? value.name : fallbackLabel;
  const generated = createGeneratedTheme(label, background, accent);
  const imported = { ...generated.roles };
  const mapped = new Set<ThemeRole>(["background"]);
  const rolePick = (role: ThemeRole, ...keys: string[]) => {
    const selected = pick(...keys);
    if (selected) {
      imported[role] = selected;
      mapped.add(role);
    }
  };
  rolePick("foreground", "editor.foreground", "foreground");
  rolePick("card", "editorWidget.background", "sideBarSectionHeader.background");
  imported.cardForeground = imported.foreground;
  if (mapped.has("foreground")) mapped.add("cardForeground");
  rolePick("popover", "menu.background", "quickInput.background", "dropdown.background");
  imported.popoverForeground = imported.foreground;
  if (mapped.has("foreground")) mapped.add("popoverForeground");
  rolePick("border", "panel.border", "editorGroup.border", "contrastBorder");
  rolePick("input", "input.background", "dropdown.background");
  rolePick("mutedForeground", "descriptionForeground", "editorLineNumber.foreground");
  rolePick("sidebar", "sideBar.background", "activityBar.background");
  rolePick("sidebarForeground", "sideBar.foreground");
  rolePick("sidebarBorder", "sideBar.border");
  const baseAnsiTokens: Array<[AnsiSlot, string]> = [
    ["black", "terminal.ansiBlack"], ["red", "terminal.ansiRed"], ["green", "terminal.ansiGreen"],
    ["yellow", "terminal.ansiYellow"], ["blue", "terminal.ansiBlue"], ["magenta", "terminal.ansiMagenta"],
    ["cyan", "terminal.ansiCyan"], ["white", "terminal.ansiWhite"],
  ];
  const importedAnsi: Partial<Record<AnsiSlot, string>> = {};
  for (const [slot, token] of baseAnsiTokens) {
    const selected = pick(token);
    if (selected) importedAnsi[slot] = selected;
  }
  let finalAnsi = generated.ansi;
  if (baseAnsiTokens.every(([slot]) => importedAnsi[slot])) {
    const brightTokens: Array<[AnsiSlot, string]> = [
      ["brightBlack", "terminal.ansiBrightBlack"], ["brightRed", "terminal.ansiBrightRed"],
      ["brightGreen", "terminal.ansiBrightGreen"], ["brightYellow", "terminal.ansiBrightYellow"],
      ["brightBlue", "terminal.ansiBrightBlue"], ["brightMagenta", "terminal.ansiBrightMagenta"],
      ["brightCyan", "terminal.ansiBrightCyan"], ["brightWhite", "terminal.ansiBrightWhite"],
    ];
    // A missing bright slot is the base hue made *more* prominent, which on a
    // light canvas means darker, not lighter.
    const emphasize = scheme === "dark" ? lighten : darken;
    for (const [slot, token] of brightTokens) {
      const selected = pick(token);
      importedAnsi[slot] = selected ?? emphasize(importedAnsi[slot.replace("bright", "").replace(/^./, (c) => c.toLowerCase()) as AnsiSlot] ?? generated.ansi[slot], 0.08);
    }
    finalAnsi = { ...generated.ansi, ...importedAnsi };
  }
  return {
    theme: {
      ...generated,
      scheme,
      roles: imported,
      ansi: finalAnsi,
      source: "vscode",
      managed: false,
    },
    mapped,
  };
}

/**
 * Parse `text` and report what the source actually carried.
 *
 * The studio parses on paste rather than behind a button, so this is the one
 * call the import panel makes on every keystroke — it must either return a
 * complete theme or throw a sentence worth showing.
 */
export function importThemeDetailed(text: string, label = "Imported theme"): ThemeImportResult {
  if (/--(?:background|foreground|primary)\s*:/.test(text)) {
    const { theme, mapped } = importShadcn(text, label);
    return { theme, source: "shadcn", ...splitRoles(mapped) };
  }
  let value: unknown;
  try {
    value = JSON.parse(stripJsonComments(text));
  } catch {
    throw new Error("Paste a Codemux theme JSON, VS Code theme JSONC, or shadcn variable block.");
  }
  if (!isRecord(value)) throw new Error("Theme files must contain a JSON object.");
  const isVsCode = isRecord(value.colors) && Object.keys(value.colors).some((key) => key.includes("."));
  if (isVsCode || Array.isArray(value.tokenColors)) {
    const { theme, mapped } = importVsCode(value, label);
    return { theme, source: "vscode", ...splitRoles(mapped) };
  }
  const parsed = parseCustomTheme(value);
  if (!parsed) throw new Error("That is not a valid Codemux theme file.");
  // A Codemux file is authored against this exact schema, so whatever it
  // names is mapped and the rest is genuinely ours.
  const declared = isRecord(value.roles) ? Object.keys(value.roles) : [];
  const mapped = new Set(THEME_ROLES.filter((role) => declared.includes(role)));
  return { theme: parsed, source: "codemux", ...splitRoles(mapped) };
}

export function importThemeText(text: string, label = "Imported theme"): ThemeDefinition {
  return importThemeDetailed(text, label).theme;
}

/** How the studio names a source in its "Recognised a …" sentence. */
export const THEME_IMPORT_SOURCE_LABEL: Readonly<Record<ThemeImportSource, string>> = {
  shadcn: "shadcn",
  vscode: "VS Code",
  codemux: "Codemux",
};

export function resolveTheme(themeId: string | null | undefined, customThemes: readonly ThemeDefinition[] = []): ThemeDefinition {
  // `"system"` and `"dark"` are the pre-palette appearance settings, still on
  // disk for anyone who never opened the theme picker. They collapse to
  // Graphite for now; a follow-up adds a real light/dark/system mode that will
  // give `"system"` its own meaning back.
  const normalized = themeId === "system" || themeId === "dark" || !themeId ? "default" : themeId;
  return customThemes.find((theme) => theme.id === normalized) ?? BUILT_IN_THEMES.find((theme) => theme.id === normalized) ?? BUILT_IN_THEMES[0]!;
}

let activeTheme: ThemeDefinition = BUILT_IN_THEMES[0]!;
const activeThemeListeners = new Set<() => void>();

export function getActiveTheme(): ThemeDefinition {
  return activeTheme;
}

export function subscribeActiveTheme(listener: () => void): () => void {
  activeThemeListeners.add(listener);
  return () => activeThemeListeners.delete(listener);
}

function notifyActiveTheme(theme: ThemeDefinition) {
  activeTheme = theme;
  for (const listener of activeThemeListeners) listener();
}

function writeThemeVariables(root: HTMLElement, theme: ThemeDefinition) {
  for (const role of THEME_ROLES) root.style.setProperty(ROLE_VARIABLES[role], theme.roles[role]);
  for (const slot of ANSI_SLOTS) root.style.setProperty(ANSI_VARIABLES[slot], theme.ansi[slot]);
  if (theme.radius) root.style.setProperty("--cm-theme-radius", theme.radius);
  else root.style.removeProperty("--cm-theme-radius");
  root.dataset.themeId = theme.id;
  // The palette owns the scheme. `.dark` still drives Tailwind's `dark:`
  // variant and the handful of scheme-specific rules in globals.css;
  // `color-scheme` is what makes native scrollbars, form controls, and the
  // window chrome follow along.
  root.dataset.themeScheme = theme.scheme;
  root.classList.toggle("dark", theme.scheme === "dark");
  root.style.colorScheme = theme.scheme;
}

function persistBootTheme(theme: ThemeDefinition) {
  try {
    window.localStorage.setItem(THEME_BOOT_STORAGE_KEY, JSON.stringify({
      id: theme.id,
      scheme: theme.scheme,
      roles: theme.roles,
      ansi: theme.ansi,
      radius: theme.radius ?? null,
    }));
  } catch {
    // Storage is best-effort. The inline boot script falls back to Graphite.
  }
}

export function applyTheme(theme: ThemeDefinition, options: { animate?: boolean; persist?: boolean } = {}) {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  if (options.animate !== false) root.classList.add("no-transitions");
  root.classList.remove("theme-warm");
  writeThemeVariables(root, theme);
  // Force the variable swap to commit while transitions are disabled.
  if (options.animate !== false) {
    void root.offsetHeight;
    requestAnimationFrame(() => root.classList.remove("no-transitions"));
  }
  if (options.persist !== false) {
    persistBootTheme(theme);
    // Previews (`persist: false`) deliberately don't repaint the OS window:
    // the palette they show is not the one the next launch will boot into.
    const background = normalizeColor(theme.roles.background);
    if (background) syncWindowBackground(background);
  }
  notifyActiveTheme(theme);
}

export function themeToSyntaxColors(theme: ThemeDefinition): ThemeColors {
  /**
   * The selection pair is the canvas nudged one step, with the palette's ink
   * on top — mirrored per scheme. On a dark canvas that is `brightBlack`
   * (the background lightened) under `white`; on a light one it is `white`
   * (the background darkened) under `black`. Taking the dark pairing on a
   * light palette would put `white` on `brightBlack` at about 3:1, because
   * both named slots keep their literal meaning whatever the canvas does.
   */
  const light = theme.scheme === "light";
  return {
    accent: normalizeColor(theme.roles.brandAccent) ?? theme.ansi.blue,
    cursor: normalizeColor(theme.roles.sidebarPrimary) ?? theme.ansi.blue,
    foreground: normalizeColor(theme.roles.foreground) ?? theme.ansi.white,
    background: normalizeColor(theme.roles.background) ?? theme.ansi.black,
    selection_foreground: light ? theme.ansi.black : theme.ansi.white,
    selection_background: light ? theme.ansi.white : theme.ansi.brightBlack,
    color0: theme.ansi.black,
    color1: theme.ansi.red,
    color2: theme.ansi.green,
    color3: theme.ansi.yellow,
    color4: theme.ansi.blue,
    color5: theme.ansi.magenta,
    color6: theme.ansi.cyan,
    color7: theme.ansi.white,
    color8: theme.ansi.brightBlack,
    color9: theme.ansi.brightRed,
    color10: theme.ansi.brightGreen,
    color11: theme.ansi.brightYellow,
    color12: theme.ansi.brightBlue,
    color13: theme.ansi.brightMagenta,
    color14: theme.ansi.brightCyan,
    color15: theme.ansi.brightWhite,
  };
}

export const THEME_ROLE_VARIABLES = ROLE_VARIABLES;
export const THEME_ANSI_VARIABLES = ANSI_VARIABLES;
