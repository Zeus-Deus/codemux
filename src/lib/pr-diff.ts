import { parseDiff, type DiffLine } from "./diff-parser";

/**
 * A whole-PR unified diff, cut into the files a reviewer reads one at a
 * time.
 *
 * `parseDiff` already turns a patch into rows and is the only diff
 * parser this app has — this module doesn't replace it, it slices the
 * text by file first and hands each slice over, so the Code tab and the
 * Changes pane keep producing identical rows from identical bytes.
 */

export type PrFileStatus = "added" | "deleted" | "renamed" | "modified";

export interface PrDiffFile {
  /** How the host addresses this file in a comment: the new path, or
   *  the old one when the file was deleted. */
  path: string;
  oldPath: string | null;
  status: PrFileStatus;
  /** The raw patch for this file, headers included. */
  patch: string;
  lines: DiffLine[];
  additions: number;
  deletions: number;
  /** No rows to render, and none to anchor a note to. */
  binary: boolean;
  /** Machine-written: listed, never rendered, never worth a line note. */
  generated: boolean;
}

/**
 * Numbers, not vibes: past this many changed lines a file is a stat
 * line with a way to ask for it, not a wall you have to scroll past to
 * reach the file you came for.
 */
export const LARGE_FILE_CHANGED_LINES = 2000;

export function changedLines(file: PrDiffFile): number {
  return file.additions + file.deletions;
}

export function isLargeFile(file: PrDiffFile): boolean {
  return !file.binary && changedLines(file) > LARGE_FILE_CHANGED_LINES;
}

/** Rendered as rows at all? Binary and generated files are listed only. */
export function isRenderable(file: PrDiffFile): boolean {
  return !file.binary && !file.generated;
}

/**
 * Files nobody reviews line by line.
 *
 * Deliberately a short, boring list rather than a heuristic on content:
 * a wrong guess here hides a real change, and "listed but not rendered"
 * is only acceptable when the reason is obvious from the path.
 */
const GENERATED_PATTERNS = [
  /(^|\/)package-lock\.json$/,
  /(^|\/)pnpm-lock\.yaml$/,
  /(^|\/)yarn\.lock$/,
  /(^|\/)Cargo\.lock$/,
  /(^|\/)bun\.lockb?$/,
  /(^|\/)composer\.lock$/,
  /(^|\/)go\.sum$/,
  /\.min\.(js|css)$/,
  /\.(snap|map)$/,
];

export function isGeneratedPath(path: string): boolean {
  return GENERATED_PATTERNS.some((re) => re.test(path));
}

function stripPrefix(p: string): string {
  if (p.startsWith("a/") || p.startsWith("b/")) return p.slice(2);
  return p;
}

/**
 * Cut a whole-PR diff into per-file patches.
 *
 * Anything before the first `diff --git` is dropped: `gh pr diff` can
 * prepend tool banners, and a banner parsed as diff content desyncs
 * every line number after it.
 */
export function splitDiffFiles(text: string): PrDiffFile[] {
  if (!text) return [];
  const files: PrDiffFile[] = [];
  // Either terminator: `gh pr diff` on a Windows host answers with CRLF,
  // and a trailing \r would ride into every path, every rename and every
  // line of content. `buildFile` re-joins with "\n", so the per-file
  // patch handed on from here is normalised whatever arrived.
  const lines = text.split(/\r?\n/);

  let start = -1;
  for (let i = 0; i <= lines.length; i++) {
    const isHeader = i < lines.length && lines[i].startsWith("diff --git ");
    if (isHeader || i === lines.length) {
      if (start >= 0) {
        const file = buildFile(lines.slice(start, i));
        if (file) files.push(file);
      }
      start = isHeader ? i : -1;
    }
  }
  return files;
}

function buildFile(chunk: string[]): PrDiffFile | null {
  const patch = chunk.join("\n");
  let oldPath: string | null = null;
  let newPath: string | null = null;
  let binary = false;
  let inHunk = false;
  let additions = 0;
  let deletions = 0;
  let renamedFrom: string | null = null;
  let renamedTo: string | null = null;

  for (const raw of chunk) {
    if (!inHunk) {
      if (raw.startsWith("--- ")) {
        const p = raw.slice(4);
        oldPath = p === "/dev/null" ? null : stripPrefix(p);
        continue;
      }
      if (raw.startsWith("+++ ")) {
        const p = raw.slice(4);
        newPath = p === "/dev/null" ? null : stripPrefix(p);
        continue;
      }
      if (raw.startsWith("rename from ")) renamedFrom = raw.slice(12);
      if (raw.startsWith("rename to ")) renamedTo = raw.slice(10);
      if (raw.startsWith("GIT binary patch") || /^Binary files .* differ$/.test(raw)) {
        binary = true;
      }
    }
    if (raw.startsWith("@@")) {
      inHunk = true;
      continue;
    }
    if (!inHunk) continue;
    if (raw.startsWith("+")) additions++;
    else if (raw.startsWith("-")) deletions++;
  }

  // `diff --git a/x b/y` is the only header a binary or mode-only change
  // is guaranteed to have.
  if (!oldPath && !newPath) {
    const m = /^diff --git a\/(.+?) b\/(.+)$/.exec(chunk[0] ?? "");
    if (m) {
      oldPath = m[1];
      newPath = m[2];
    }
  }
  oldPath ??= renamedFrom;
  newPath ??= renamedTo;

  const path = newPath ?? oldPath;
  if (!path) return null;

  const status: PrFileStatus = !newPath
    ? "deleted"
    : !oldPath
      ? "added"
      : oldPath !== newPath
        ? "renamed"
        : "modified";

  return {
    path,
    oldPath,
    status,
    patch,
    lines: binary ? [] : parseDiff(patch),
    additions,
    deletions,
    binary,
    generated: isGeneratedPath(path),
  };
}

/** Two lines that differ only in spacing. */
function sameIgnoringWhitespace(a: string, b: string): boolean {
  return a.replace(/\s+/g, "") === b.replace(/\s+/g, "");
}

/**
 * Fold whitespace-only changes back into context.
 *
 * Done here rather than by re-asking the host for `?w=1`: the diff we
 * render must be the same bytes the anchors were computed against, and
 * a second, differently-numbered diff arriving from the network is the
 * fastest way to pin a note to the wrong line. A deleted/added pair that
 * differs only in spacing becomes one context row carrying both line
 * numbers, so every other row keeps its coordinates.
 */
export function ignoreWhitespace(lines: DiffLine[]): DiffLine[] {
  const out: DiffLine[] = [];
  let i = 0;
  while (i < lines.length) {
    if (lines[i].type !== "del") {
      out.push(lines[i]);
      i++;
      continue;
    }
    const dels: DiffLine[] = [];
    while (i < lines.length && lines[i].type === "del") dels.push(lines[i++]);
    const adds: DiffLine[] = [];
    while (i < lines.length && lines[i].type === "add") adds.push(lines[i++]);

    // Only a clean pairwise match is safe to fold; an uneven block means
    // real edits are mixed in and the rows stay as they are.
    const foldable =
      dels.length === adds.length &&
      dels.every((d, j) => sameIgnoringWhitespace(d.content, adds[j].content));
    if (foldable) {
      for (let j = 0; j < adds.length; j++) {
        out.push({
          type: "context",
          content: adds[j].content,
          oldLine: dels[j].oldLine,
          newLine: adds[j].newLine,
        });
      }
    } else {
      out.push(...dels, ...adds);
    }
  }
  return out;
}
