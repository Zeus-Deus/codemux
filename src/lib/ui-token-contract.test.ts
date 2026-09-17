import { readFileSync, readdirSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * UI token contract — a budget ratchet.
 *
 * The design system is not missing; it gets bypassed. A raw `text-sm`
 * renders identically today and then refuses to move when the user changes
 * the interface size; a `rounded-[7px]` renders identically and then
 * ignores the theme's radius. Neither shows up in a screenshot, a type
 * check or a behavioural test, which is why they accumulated to 543 and 256
 * sites respectively before anyone counted.
 *
 * This is the same idiom as `theme-color-contract.test.ts`: a
 * filesystem-scanning Vitest test rather than a lint rule, because the repo
 * has no ESLint and one rule does not justify adding a toolchain. It runs
 * inside the already-required `npm run test`, on both OSes, with no new CI
 * step and no new dependency.
 *
 * Each category carries a *budget*: the number of known, justified
 * occurrences. Exceeding it fails and names the files. Lowering a budget
 * when you remove the last one is the point — it ratchets.
 */

/** Every string literal in a source file: where classes actually live.
 *  Scanning literals rather than raw text keeps the word "rounded" in a
 *  comment from counting as a bypassed radius. */
function stringLiterals(contents: string): string[] {
  return contents.match(/"[^"\n]*"|'[^'\n]*'|`[^`]*`/g) ?? [];
}

/** Opening `<Button …>` tags, including multi-line ones. */
function buttonTags(contents: string): string[] {
  return contents.match(/<Button\b[\s\S]*?>/g) ?? [];
}

interface Category {
  /** What the pattern is looking for, in the failure message. */
  label: string;
  /** Matches counted inside string literals, unless `scan` says otherwise. */
  pattern: RegExp;
  /** Known, justified occurrences. Never raise this to make a diff pass. */
  budget: number;
  /** Why the budget is not zero. Empty when it is. */
  why?: string;
  /** Narrow the text each file contributes (default: its string literals). */
  scan?: (contents: string) => string[];
}

const CATEGORIES: Category[] = [
  {
    label: "raw Tailwind type sizes (use text-label / text-body / text-body-lg)",
    pattern: /(?<![-\w:])text-(?:xs|sm|base)\b/g,
    budget: 0,
  },
  {
    label: "bare `rounded` (compiles to a hard 4px; use rounded-sm)",
    pattern: /(?<![-\w])rounded(?![-\w])/g,
    budget: 0,
  },
  {
    label: "arbitrary pixel radii (use the sm/md/lg ladder)",
    pattern: /rounded(?:-[tblrse]{1,2})?-\[[0-9]/g,
    budget: 6,
    why:
      "the five sanctioned carve-outs: the two asymmetric chat-bubble tail " +
      "radii (3 sites) and the composer strip/pill seam (the 22px pill plus " +
      "the two 14px seam halves above and below it)",
  },
  {
    label: "improvised surface alphas (use bg-surface-1/2/3)",
    // Only the surface range. A `bg-foreground/90` is ink — an inverted
    // button, a status dot, a meter bar — and the ladder tops out at 9.5%.
    pattern: /bg-foreground\/(?:\[0\.(?:0\d+|1[0-5]\d*)\]|[0-9]|1[0-5])(?![\d.])/g,
    budget: 0,
  },
  {
    label: "Button call sites patching their own geometry",
    pattern: /className="[^"]*\b(?:h-|px-|py-|rounded-(?:sm|md|lg)\b)/g,
    scan: buttonTags,
    budget: 0,
  },
  {
    label: "icons spelled `h-N w-N` instead of `size-N`",
    pattern: /\bh-(?:3|3\.5|4|4\.5) w-(?:3|3\.5|4|4\.5)(?![\d/.])/g,
    budget: 0,
  },
  {
    label: "transitions with no explicit duration (they silently inherit 150ms)",
    pattern:
      /"[^"\n]*\btransition-(?:colors|opacity|transform|all|\[)(?![^"\n]*\bduration-)[^"\n]*"/g,
    scan: (contents) => [contents],
    budget: 0,
  },
  {
    label: "durations off the 100 / 150 / 250 ladder",
    pattern: /(?<![-\w])duration-(?!100\b|150\b|250\b|0\b)[\w[\]]+/g,
    budget: 6,
    why:
      "progress and meter fills, which animate a *value* rather than a UI " +
      "state: the context-usage sweep (500ms), three width fills and the " +
      "clone/sweep bars (300ms)",
  },
  {
    label: "per-icon strokeWidth (one base-layer rule owns icon weight)",
    pattern: /strokeWidth/g,
    scan: (contents) => [contents],
    budget: 11,
    why:
      "hand-written <svg> glyphs and chart geometry, which are not lucide " +
      "icons: two device glyphs, two goal glyphs, the context meter's two " +
      "arcs and the usage chart's five strokes",
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

describe("UI token contract", () => {
  it.each(CATEGORIES)("stays within budget: $label", (category) => {
    const offenders: string[] = [];
    for (const file of files) {
      const contents = readFileSync(file, "utf8");
      const chunks = category.scan
        ? category.scan(contents)
        : stringLiterals(contents);
      for (const chunk of chunks) {
        for (const match of chunk.matchAll(category.pattern)) {
          offenders.push(`${relative(root, file)}: ${match[0].slice(0, 60)}`);
        }
      }
    }
    // The message, not just the number: a failure has to point at the file.
    expect(
      offenders.length,
      offenders.length > category.budget
        ? `${offenders.length} occurrences, budget ${category.budget}` +
            `${category.why ? ` (${category.why})` : ""}\n  ` +
            offenders.slice(0, 20).join("\n  ")
        : "",
    ).toBeLessThanOrEqual(category.budget);
  });

  it("keeps one real focus ring in the base layer", () => {
    // The rule this replaced set only a colour (`outline-ring/50` on `*`),
    // so it painted nothing and the app shipped 49 controls that were
    // unreachable by keyboard. A colour with no width is the bug.
    const css = readFileSync(resolve(root, "src/globals.css"), "utf8");
    const rule = css.match(/:focus-visible\s*\{[^}]*\}/)?.[0] ?? "";
    expect(rule).toMatch(/outline:\s*\d/);
    expect(rule).toMatch(/outline-offset/);
    expect(css).not.toMatch(/@apply[^;]*outline-ring/);
  });
});
