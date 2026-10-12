/**
 * Review comments as their authors wrote them: Markdown, with GitHub's
 * ```suggestion fences pulled out so they can be shown as a change and
 * applied rather than read as a wall of backticks.
 */

import { rowsFor, type AnchorIndex, type AnchorSide } from "./pr-anchor";

export type CommentSegment =
  | { kind: "markdown"; text: string }
  | { kind: "suggestion"; lines: string[] };

const SUGGESTION_OPEN = /^ {0,3}(`{3,}|~{3,})\s*suggestion\s*$/;

/**
 * Split a comment body into prose and suggestion blocks, in order.
 *
 * An unclosed fence runs to the end of the comment, the way GitHub
 * renders it. A suggestion with no lines in it is a real suggestion: it
 * proposes deleting the anchored lines.
 */
export function splitSuggestions(body: string): CommentSegment[] {
  const lines = body.replace(/\r\n/g, "\n").split("\n");
  const out: CommentSegment[] = [];
  let prose: string[] = [];
  const flush = () => {
    const text = prose.join("\n");
    if (text.trim()) out.push({ kind: "markdown", text });
    prose = [];
  };

  let i = 0;
  while (i < lines.length) {
    const open = SUGGESTION_OPEN.exec(lines[i]);
    if (!open) {
      prose.push(lines[i]);
      i++;
      continue;
    }
    const fence = open[1];
    const close = new RegExp(`^ {0,3}\\${fence[0]}{${fence.length},}\\s*$`);
    const suggested: string[] = [];
    let j = i + 1;
    while (j < lines.length && !close.test(lines[j])) {
      suggested.push(lines[j]);
      j++;
    }
    flush();
    out.push({ kind: "suggestion", lines: suggested });
    i = j + 1;
  }
  flush();
  return out;
}

export function hasSuggestion(body: string): boolean {
  return splitSuggestions(body).some((s) => s.kind === "suggestion");
}

/**
 * The lines `start..end` on one side of one file, read off the diff, or
 * null when any of them is not in it. A suggestion can only be shown as a
 * change, or applied, against lines we actually have.
 */
export function originalLines(
  index: AnchorIndex,
  path: string,
  side: AnchorSide,
  start: number,
  end: number,
): string[] | null {
  if (start > end) return null;
  const rows = rowsFor(index, path, side);
  const out: string[] = [];
  for (let n = start; n <= end; n++) {
    const row = rows.find((r) => r.line === n);
    if (!row) return null;
    out.push(row.text);
  }
  return out;
}

/**
 * Replace lines `start..end` (1-based, inclusive) of `text` with
 * `replacement`, but only if they still read `original`.
 *
 * The check is the whole point: a suggestion was written against the
 * pull request's head, and a checkout with local edits on those lines
 * would otherwise have someone else's change spliced into the middle of
 * its own. The file's line ending is kept.
 */
export function applySuggestion(
  text: string,
  start: number,
  end: number,
  original: string[],
  replacement: string[],
): string {
  const eol = text.includes("\r\n") ? "\r\n" : "\n";
  const lines = text.split(eol);
  const current = lines.slice(start - 1, end);
  const matches =
    current.length === original.length && current.every((line, i) => line === original[i]);
  if (!matches) {
    throw new Error(
      "Those lines have changed in your checkout since the suggestion was written",
    );
  }
  lines.splice(start - 1, end - start + 1, ...replacement);
  return lines.join(eol);
}
