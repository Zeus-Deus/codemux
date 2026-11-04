import { readFileSync, readdirSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { BUILT_IN_THEMES, contrastRatio, normalizeColor, subtleForeground } from "./themes";

// Text faded below the subtle ink floor composites under 4.5:1 and reads as
// disabled. `text-subtle-foreground` is the quietest legal text ink.
const FADED_TEXT_INK = /(?<![\w-])text-(?:muted-foreground\/(?:[1-7]\d|[0-9])|foreground\/(?:[1-4]\d|[0-9]))\b/g;

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

  it.each(BUILT_IN_THEMES.map((theme) => [theme.id, theme] as const))(
    "%s keeps muted and subtle ink readable and the focus ring visible on every surface",
    (_id, theme) => {
      const { roles } = theme;
      const backdrop = normalizeColor(roles.background) ?? "#000000";
      const surfaces = [roles.background, roles.sidebar, roles.card, roles.popover]
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

