import { readFileSync, readdirSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * Drift ratchets that sit beside `ui-token-contract.test.ts`.
 *
 * That contract polices the spellings it was written for (`bg-foreground/…`,
 * `text-sm`, `h-4 w-4`). The same drift kept arriving through the gaps: a
 * hover spelled `hover:bg-muted/40` instead of `bg-foreground/[0.04]`, a type
 * size spelled `text-[11px]` instead of `text-sm`, an icon spelled
 * `size-[13px]`. Each category here counts one of those spellings and fails
 * when the count rises, so a migration that clears some of them stays
 * cleared. The budgets are today's counts, not targets: lower one whenever
 * you remove occurrences, and never raise one to make a diff pass. The
 * alpha-fill budget carries six more than today's 188 so branches that were
 * already open when it was set can land; tighten it once they have.
 */

/**
 * Quoted and backticked text outside comments. Comments are matched in the
 * same alternation and dropped, so a docstring that quotes a class name does
 * not move a budget, while a `//` inside a string (a URL) is left alone
 * because the string starts first.
 */
function stringLiterals(contents: string): string[] {
  const tokens =
    contents.match(
      /\/\*[\s\S]*?\*\/|\/\/[^\n]*|"[^"\n]*"|'[^'\n]*'|`[^`]*`/g,
    ) ?? [];
  return tokens.filter((token) => !token.startsWith("/"));
}

interface Category {
  label: string;
  pattern: RegExp;
  budget: number;
  why: string;
}

const CATEGORIES: Category[] = [
  {
    label:
      "improvised muted/accent/secondary alpha fills (use bg-surface-1/2/3)",
    // `bg-muted/40`, `hover:bg-accent/50`, `data-[selected=true]:bg-muted/70`.
    pattern: /(?<![-\w])bg-(?:muted|accent|secondary)\/\d+/g,
    budget: 194,
    why:
      "not yet migrated: hover fills map to surface-2, resting cards to " +
      "surface-1, selected rows to surface-3",
  },
  {
    label: "bare hover:bg-muted (the hover step is hover:bg-surface-2)",
    pattern: /(?<![-\w])hover:bg-muted(?![-/\w])/g,
    budget: 20,
    why: "not yet migrated to hover:bg-surface-2",
  },
  {
    label: "arbitrary px/rem type sizes (use the text-micro…text-body-lg scale)",
    // `em` is left out: a preview that scales with its own container uses it
    // on purpose.
    pattern: /(?<![-\w])text-\[\d+(?:\.\d+)?(?:px|rem)\]/g,
    budget: 11,
    why:
      "display titles with no token yet (the scale tops out at text-body-lg) " +
      "and dense badges still to migrate",
  },
  {
    label: "arbitrary px icon and glyph boxes (use size-3 / size-3.5 / size-4)",
    pattern:
      /(?<![-\w])(?:size-\[\d+(?:\.\d+)?px\]|h-\[(\d+(?:\.\d+)?)px\] w-\[\1px\])/g,
    budget: 84,
    why:
      "status dots and swatches (true circles are carved out) plus icons " +
      "still to snap to the three-size ladder",
  },
];

function sourceFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return /\.(?:ts|tsx)$/.test(entry.name) && !entry.name.includes(".test.")
      ? [path]
      : [];
  });
}

const root = resolve(process.cwd());
const files = sourceFiles(resolve(root, "src"));

describe("UI token drift", () => {
  it.each(CATEGORIES)("stays within budget: $label", (category) => {
    const offenders: string[] = [];
    for (const file of files) {
      for (const chunk of stringLiterals(readFileSync(file, "utf8"))) {
        for (const match of chunk.matchAll(category.pattern)) {
          offenders.push(`${relative(root, file)}: ${match[0]}`);
        }
      }
    }
    expect(
      offenders.length,
      offenders.length > category.budget
        ? `${offenders.length} occurrences, budget ${category.budget} ` +
            `(${category.why})\n  ${offenders.slice(0, 20).join("\n  ")}`
        : "",
    ).toBeLessThanOrEqual(category.budget);
  });
});
