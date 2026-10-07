import type { IBufferLine, IBufferRange, Terminal } from "@xterm/xterm";

import { isAbsolute, normalizePath } from "@/lib/agent-chat/file-links";

/** A link found in one logical (unwrapped) terminal line. Offsets are
 *  UTF-16 indices into that line's text, end exclusive. */
export type TerminalLinkMatch =
  | { kind: "url"; start: number; end: number; url: string }
  | {
      kind: "file";
      start: number;
      end: number;
      path: string;
      line?: number;
      column?: number;
    };

const URL_RE = /\bhttps?:\/\/[^\s"'`<>]+/g;

// A path needs either a directory separator or a line suffix: a bare
// `name.ext` with neither is far more often prose ("Node.js", "e.g.") than a
// file. Extensions start with a letter so version numbers never match.
// Suffixes: `:12`, `:12:5` (most tools) and `(12,5)` (tsc). Windows output
// uses `\` separators and a drive prefix (`C:\src\main.rs:12`).
const FILE_RE =
  /(?<![\w.~/@+-])((?:[A-Za-z]:[\\/]|\.{1,2}[\\/]|\/)?(?:[\w.@+-]+[\\/])*[\w@+-][\w.@+-]*\.[A-Za-z][A-Za-z0-9]{0,9})(?::(\d+)(?::(\d+))?|\((\d+),(\d+)\))?/g;

/** Punctuation that ends a sentence rather than a URL. */
const URL_TRAILING = /[.,;:!?'")\]}>]+$/;

function trimUrl(raw: string): string {
  let url = raw.replace(URL_TRAILING, "");
  // Keep a closing paren that balances one inside the URL (Wikipedia-style).
  const tail = raw.slice(url.length);
  if (tail.startsWith(")")) {
    const open = (url.match(/\(/g) ?? []).length;
    const close = (url.match(/\)/g) ?? []).length;
    if (open > close) url += ")";
  }
  return url;
}

/** Find URLs and `path[:line[:column]]` references in a line of terminal text. */
export function findTerminalLinks(text: string): TerminalLinkMatch[] {
  const links: TerminalLinkMatch[] = [];
  for (const m of text.matchAll(URL_RE)) {
    const url = trimUrl(m[0]);
    if (url.length <= "https://".length) continue;
    links.push({ kind: "url", start: m.index, end: m.index + url.length, url });
  }
  const insideUrl = (index: number) =>
    links.some((link) => index >= link.start && index < link.end);

  for (const m of text.matchAll(FILE_RE)) {
    if (insideUrl(m.index)) continue;
    const path = m[1];
    const line = m[2] ?? m[4];
    const column = m[3] ?? m[5];
    if (!line && !/[\\/]/.test(path)) continue;
    links.push({
      kind: "file",
      start: m.index,
      end: m.index + m[0].length,
      path,
      line: line ? Number(line) : undefined,
      column: column ? Number(column) : undefined,
    });
  }
  return links.sort((a, b) => a.start - b.start);
}

/** Resolve a path printed in a terminal against the pane's working directory.
 *  Returns null for a relative path when the directory is unknown. */
export function resolveTerminalPath(path: string, cwd: string | null | undefined): string | null {
  const absolute = isAbsolute(path);
  if (!absolute && !cwd) return null;
  return normalizePath(absolute ? path : `${cwd}/${path}`);
}

/** The loopback hosts a dev server prints. These open inside Codemux. */
export function isLoopbackUrl(url: string): boolean {
  try {
    const host = new URL(url).hostname.replace(/^\[|\]$/g, "");
    return (
      host === "localhost" ||
      host.endsWith(".localhost") ||
      host === "0.0.0.0" ||
      host === "::1" ||
      /^127\.\d+\.\d+\.\d+$/.test(host)
    );
  } catch {
    return false;
  }
}

/**
 * The logical line containing buffer row `row` (0-based): the text of every
 * soft-wrapped row joined together, plus where each UTF-16 unit sits on
 * screen. Built cell by cell so wide glyphs (CJK, emoji) keep their columns.
 */
export function readLogicalLine(
  term: Pick<Terminal, "buffer">,
  row: number,
): { text: string; cells: { x: number; y: number; width: number }[] } {
  const buffer = term.buffer.active;
  let first = row;
  while (first > 0 && buffer.getLine(first)?.isWrapped) first--;
  let last = row;
  while (buffer.getLine(last + 1)?.isWrapped) last++;

  let text = "";
  const cells: { x: number; y: number; width: number }[] = [];
  for (let y = first; y <= last; y++) {
    const line: IBufferLine | undefined = buffer.getLine(y);
    if (!line) break;
    for (let x = 0; x < line.length; x++) {
      const cell = line.getCell(x);
      if (!cell) continue;
      const width = cell.getWidth();
      if (width === 0) continue;
      const chars = cell.getChars() || " ";
      for (let i = 0; i < chars.length; i++) cells.push({ x, y, width });
      text += chars;
    }
  }
  return { text, cells };
}

/** Screen range (1-based, inclusive) for a match within a logical line. */
export function rangeForMatch(
  cells: { x: number; y: number; width: number }[],
  start: number,
  end: number,
): IBufferRange | null {
  const from = cells[start];
  const to = cells[end - 1];
  if (!from || !to) return null;
  return {
    start: { x: from.x + 1, y: from.y + 1 },
    end: { x: to.x + to.width, y: to.y + 1 },
  };
}
