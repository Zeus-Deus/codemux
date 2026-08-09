import { readFileSync, readdirSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { describe, expect, it } from "vitest";

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
});
