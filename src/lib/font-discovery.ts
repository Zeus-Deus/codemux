import { normalizeFontFamily, quoteFontFamily } from "./typography";

const BUNDLED_FONTS = ["DM Sans Variable", "JetBrains Mono Variable"] as const;

// A fallback catalog for WebKitGTK / WKWebView, where Local Font Access is
// unavailable. Metric probing filters this to faces that really render on the
// device; users can still type any installed family into the picker.
const CURATED_FONTS = [
  "Atkinson Hyperlegible",
  "Avenir Next",
  "Berkeley Mono",
  "Cascadia Code",
  "Cascadia Mono",
  "Consolas",
  "DejaVu Sans",
  "DejaVu Sans Mono",
  "Fira Code",
  "Fira Mono",
  "Geist",
  "Geist Mono",
  "Hack",
  "Helvetica Neue",
  "IBM Plex Mono",
  "IBM Plex Sans",
  "Inconsolata",
  "Iosevka",
  "JetBrains Mono",
  "Menlo",
  "Monaco",
  "Noto Sans",
  "Noto Sans Mono",
  "Roboto Mono",
  "Segoe UI",
  "SF Mono",
  "Source Code Pro",
  "Source Sans 3",
  "Ubuntu",
  "Ubuntu Mono",
] as const;

const GENERIC_FONTS = ["system-ui", "ui-monospace", "sans-serif", "monospace"] as const;
const PROBE_TEXT = "mmmmmmmmMMWli1O0@# fjord";
let probeContext: CanvasRenderingContext2D | null | undefined;

function context(): CanvasRenderingContext2D | null {
  if (probeContext === undefined) {
    try {
      probeContext = document.createElement("canvas").getContext("2d");
    } catch {
      probeContext = null;
    }
  }
  return probeContext;
}

function measuredWidth(fontFamily: string): number | null {
  const ctx = context();
  if (!ctx) return null;
  ctx.font = `32px ${fontFamily}`;
  return ctx.measureText(PROBE_TEXT).width;
}

/** Metric probing avoids document.fonts.check(), which reports true when an
 * unknown family simply falls back and therefore cannot actually detect it. */
export function isFontFamilyAvailable(family: string): boolean {
  const normalized = normalizeFontFamily(family);
  if (!normalized) return false;
  if ([...GENERIC_FONTS, ...BUNDLED_FONTS].some((known) => known === normalized)) return true;
  try {
    const quoted = quoteFontFamily(normalized);
    return ["monospace", "serif", "sans-serif"].some((fallback) => {
      const baseline = measuredWidth(`"__codemux_missing_font__", ${fallback}`);
      const candidate = measuredWidth(`${quoted}, ${fallback}`);
      return baseline !== null && candidate !== null && Math.abs(baseline - candidate) > 0.01;
    });
  } catch {
    return false;
  }
}

const MONO_GLYPHS = ["i", "M", "W", "0", "@", "#", ".", " "] as const;

export function isMonospaceFont(family: string): boolean {
  const normalized = normalizeFontFamily(family);
  if (!normalized) return false;
  if (normalized === "ui-monospace" || normalized === "monospace") return true;
  const ctx = context();
  if (!ctx) return true;
  try {
    ctx.font = `32px ${quoteFontFamily(normalized)}, monospace`;
    const widths = MONO_GLYPHS.map((glyph) => ctx.measureText(glyph).width);
    const first = widths[0];
    if (!first || widths.some((width) => !Number.isFinite(width) || width <= 0)) return true;
    return widths.every((width) => Math.abs(width - first) < 0.01);
  } catch {
    return true;
  }
}

export function normalizeDiscoveredFamilies(families: readonly string[]): string[] {
  const unique = new Map<string, string>();
  for (const input of [...BUNDLED_FONTS, ...GENERIC_FONTS, ...families]) {
    const family = normalizeFontFamily(input);
    if (!family) continue;
    const key = family.toLocaleLowerCase();
    if (!unique.has(key)) unique.set(key, family);
  }
  return [...unique.values()].sort((a, b) => a.localeCompare(b));
}

type LocalFontRecord = { family?: unknown };
type LocalFontQuery = () => Promise<readonly LocalFontRecord[]>;

let discoveryPromise: Promise<readonly string[]> | null = null;

/**
 * Enumerate every installed family where Chromium exposes Local Font Access;
 * otherwise return a metric-probed catalog. The result is cached across all
 * four pickers so opening Advanced never repeats a permission request.
 */
export function discoverInstalledFontFamilies(): Promise<readonly string[]> {
  if (discoveryPromise) return discoveryPromise;
  discoveryPromise = (async () => {
    const query = (window as unknown as { queryLocalFonts?: LocalFontQuery }).queryLocalFonts;
    if (typeof query === "function") {
      try {
        const records = await query.call(window);
        return normalizeDiscoveredFamilies(
          records.flatMap((record) =>
            typeof record.family === "string" ? [record.family] : [],
          ),
        );
      } catch {
        // Permission denied is a normal outcome. The probed catalog below is
        // still useful and never opens another browser permission prompt.
      }
    }
    return normalizeDiscoveredFamilies(
      CURATED_FONTS.filter((family) => isFontFamilyAvailable(family)),
    );
  })();
  return discoveryPromise;
}
