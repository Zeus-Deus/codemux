import { readFileSync, readdirSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { BUILT_IN_THEMES, contrastRatio, normalizeColor, subtleForeground } from "./themes";

// Faded muted ink composites under 4.5:1 (and below the subtle floor on
// selected fills) and reads as disabled. `text-subtle-foreground` is the
// quietest legal text ink; foreground keeps its alphas down to 50%. Status
// and semantic tones are tuned per scheme to clear 4.5:1 only at full
// strength, so they take no alpha either.
const FADED_TEXT_INK = /(?<![\w-])text-(?:(?:muted-foreground|success|danger|warning|destructive|status-[a-z]+)\/\d+|foreground\/(?:[1-4]\d|[0-9]))\b/g;

// Fading a text span with opacity bypasses the ink floor the same way an
// alpha does. A lone separator glyph (·, ←, →) is decoration and may fade;
// words, numbers and interpolated values may not. Class strings assembled
// with cn() are out of reach of this check.
const FADED_TEXT_SPAN = /<span className="[^"]*(?<![\w:/-])opacity-[1-9]\d\b[^"]*">\s*(?:\{|[\p{L}\p{N}])/gu;

const RAW_PALETTE_UTILITY = /\b(?:bg|text|border|ring|from|to|via)-(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-\d+/g;

function sourceFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return /\.(?:ts|tsx)$/.test(entry.name) && !entry.name.includes(".test.") ? [path] : [];
  });
}

describe("theme color contract", () => {
  it("contains no raw Tailwind palette utilities in production components", () => {
    const root = resolve(process.cwd());
    const hits = sourceFiles(resolve(root, "src")).flatMap((file) => {
      const contents = readFileSync(file, "utf8");
      return [...contents.matchAll(RAW_PALETTE_UTILITY)].map((match) =>
        `${relative(root, file)}:${match[0]}`,
      );
    });
    expect(hits).toEqual([]);
  });

  it("never fades text ink below the subtle floor", () => {
    const root = resolve(process.cwd());
    const hits = sourceFiles(resolve(root, "src")).flatMap((file) => {
      const contents = readFileSync(file, "utf8");
      return [...contents.matchAll(FADED_TEXT_INK)].map((match) =>
        `${relative(root, file)}:${match[0]}`,
      );
    });
    expect(hits).toEqual([]);
  });

  it("never fades a text span with opacity", () => {
    const root = resolve(process.cwd());
    const hits = sourceFiles(resolve(root, "src")).flatMap((file) => {
      const contents = readFileSync(file, "utf8");
      return [...contents.matchAll(FADED_TEXT_SPAN)].map((match) =>
        `${relative(root, file)}:${match[0]}`,
      );
    });
    expect(hits).toEqual([]);
  });

  it.each(BUILT_IN_THEMES.map((theme) => [theme.id, theme] as const))(
    "%s keeps muted and subtle ink readable and the focus ring visible on every surface, selected fills included",
    (_id, theme) => {
      const { roles } = theme;
      const backdrop = normalizeColor(roles.background) ?? "#000000";
      const surfaces = [roles.background, roles.sidebar, roles.card, roles.popover, roles.accent, roles.sidebarAccent]
        .map((surface) => normalizeColor(surface, backdrop) ?? "#000000");
      const subtle = subtleForeground(roles);
      for (const surface of surfaces) {
        // Muted ink is also the keyboard focus outline, so 4.5:1 text
        // contrast covers the 3:1 non-text floor for the ring as well.
        expect(contrastRatio(roles.mutedForeground, surface)).toBeGreaterThanOrEqual(4.5);
        expect(contrastRatio(subtle, surface)).toBeGreaterThanOrEqual(4.5);
      }
    },
  );
});

