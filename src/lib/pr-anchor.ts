/**
 * Re-anchoring local draft review comments across a force-push.
 *
 * A TypeScript port of the validated prototype (`reanchor.py`) whose
 * verdicts were checked against a real force-push on a real repository:
 * on that experiment this matcher agreed with GitHub's own re-anchoring
 * line for line. The fixtures from that run are checked in beside the
 * test, so the agreement is a thing CI keeps, not a thing we remember.
 *
 * The rule the whole file exists to protect: a note is either pinned to
 * a line we are sure about, or it is labelled as unpinned. It is never
 * quietly moved to a line that merely looks close. Submitting a comment
 * against a line the author didn't write is worse than telling you the
 * anchor was lost.
 */

export type AnchorSide = "LEFT" | "RIGHT";

/** One addressable row of a diff, on one side. */
export interface AnchorRow {
  /** Side-relative file line number: new-file for RIGHT, old for LEFT. */
  line: number;
  text: string;
  /** The `@@ … @@` header this row lives under. */
  hunk: string;
}

/** Rows of a diff, keyed by path and side. */
export type AnchorIndex = Map<string, AnchorRow[]>;

/** What a draft remembers about where it was written. */
export interface DraftAnchor {
  path: string;
  side: AnchorSide;
  line: number;
  /** The exact text of the anchored line, sigil stripped. */
  text: string;
  contextBefore?: string | null;
  contextAfter?: string | null;
}

export type ReanchorResult =
  | { status: "reanchored"; line: number; hunk: string; moved: boolean }
  | { status: "lost"; reason: string };

const HUNK_RE = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

function keyOf(path: string, side: AnchorSide): string {
  return `${path}\u0000${side}`;
}

/**
 * Index a unified diff into per-(path, side) rows.
 *
 * RIGHT is context + added lines in new-file numbering; LEFT is context
 * + deleted lines in old-file numbering — the same coordinates GitHub's
 * `line`/`side` pair uses, so a match here is directly submittable.
 *
 * Two deliberate departures from the prototype, both of which produce
 * identical verdicts on the captured fixtures:
 *
 * - `+++`/`---` are only treated as file headers *outside* a hunk body.
 *   Inside one they are content (a deleted `-- x` renders as `--- x`),
 *   and skipping those desyncs every line number after them.
 * - A deleted file (`+++ /dev/null`) keeps its LEFT rows under the old
 *   path instead of being dropped, because a note on a line you deleted
 *   is exactly the note worth keeping.
 */
export function indexDiffRows(text: string): AnchorIndex {
  const index: AnchorIndex = new Map();
  if (!text) return index;

  let oldPath: string | null = null;
  let newPath: string | null = null;
  let oldNo = 0;
  let newNo = 0;
  let hunk: string | null = null;

  const push = (path: string | null, side: AnchorSide, row: AnchorRow) => {
    if (!path) return;
    const k = keyOf(path, side);
    const rows = index.get(k);
    if (rows) rows.push(row);
    else index.set(k, [row]);
  };

  // Split on either terminator. A diff can arrive with CRLF — a Windows
  // checkout of a fixture, or `gh` output on a Windows host — and every
  // decision below is made on the exact characters of a line: the `+`/`-`
  // prefixes, the `@@` header, a blank context line that must be `""`,
  // and the file paths that become index keys. A stray trailing \r turns
  // the path key into one nothing looks up and ends the hunk body at the
  // first blank line, which silently renumbers everything after it.
  //
  // A diff that ends in a newline splits into a final `""`. That empty
  // string is not a row — but the blank-context rule below would index it
  // as one, inventing a context line one past the end of the last hunk on
  // both sides. Drop exactly one trailing `""`; interior blanks are real
  // blank context and must stay.
  const lines = text.split(/\r?\n/);
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();

  for (const raw of lines) {
    if (raw.startsWith("diff --git ")) {
      oldPath = null;
      newPath = null;
      hunk = null;
      continue;
    }
    if (!hunk && raw.startsWith("--- ")) {
      const p = raw.slice(4);
      oldPath = p === "/dev/null" ? null : stripPathPrefix(p);
      continue;
    }
    if (!hunk && raw.startsWith("+++ ")) {
      const p = raw.slice(4);
      newPath = p === "/dev/null" ? null : stripPathPrefix(p);
      continue;
    }

    const m = HUNK_RE.exec(raw);
    if (m) {
      oldNo = parseInt(m[1], 10);
      newNo = parseInt(m[2], 10);
      hunk = raw;
      continue;
    }
    if (!hunk) continue;

    // GitHub addresses a comment by the file's current path; a deleted
    // file has none, so its LEFT rows fall back to the old path.
    const rightPath = newPath;
    const leftPath = newPath ?? oldPath;

    if (raw.startsWith("+")) {
      push(rightPath, "RIGHT", { line: newNo, text: raw.slice(1), hunk });
      newNo++;
    } else if (raw.startsWith("-")) {
      push(leftPath, "LEFT", { line: oldNo, text: raw.slice(1), hunk });
      oldNo++;
    } else if (raw.startsWith(" ") || raw === "") {
      // A blank context line is a single space, but anything that trims
      // trailing whitespace turns it into nothing at all. Treating that
      // as the end of the hunk would renumber every line below it, so
      // an empty line inside a hunk body counts as blank context.
      const text = raw.slice(1);
      push(rightPath, "RIGHT", { line: newNo, text, hunk });
      push(leftPath, "LEFT", { line: oldNo, text, hunk });
      oldNo++;
      newNo++;
    } else if (raw.startsWith("\\")) {
      // "\ No newline at end of file" — metadata, not a row.
      continue;
    } else {
      // Anything else ends the hunk body (index/mode lines, the next
      // file's header, trailing prose).
      hunk = null;
    }
  }

  return index;
}

/** `a/src/x.ts` → `src/x.ts`; leaves already-bare paths alone. */
function stripPathPrefix(p: string): string {
  if (p.startsWith("a/") || p.startsWith("b/")) return p.slice(2);
  return p;
}

/** Rows for one side of one file, in diff order. */
export function rowsFor(
  index: AnchorIndex,
  path: string,
  side: AnchorSide,
): AnchorRow[] {
  return index.get(keyOf(path, side)) ?? [];
}

/**
 * Everything a draft needs to remember about a line, read off the diff
 * it was written against.
 */
export function anchorContext(
  index: AnchorIndex,
  path: string,
  side: AnchorSide,
  line: number,
): { text: string; contextBefore: string | null; contextAfter: string | null; hunk: string } | null {
  const rows = rowsFor(index, path, side);
  const i = rows.findIndex((r) => r.line === line);
  if (i < 0) return null;
  return {
    text: rows[i].text,
    contextBefore: i > 0 ? rows[i - 1].text : null,
    contextAfter: i + 1 < rows.length ? rows[i + 1].text : null,
    hunk: rows[i].hunk,
  };
}

/**
 * Where this anchor lives in the new diff, or why we can't say.
 *
 * Exact text match within (path, side); one hit re-anchors, none is
 * lost. Several hits are narrowed by the immediate neighbours and then
 * by distance from the old line — and a distance tie is reported lost,
 * because two equally plausible lines means we don't know which one you
 * meant.
 */
export function reanchor(anchor: DraftAnchor, index: AnchorIndex): ReanchorResult {
  const rows = rowsFor(index, anchor.path, anchor.side);
  const want = anchor.text.replace(/\s+$/, "");

  let candidates: number[] = [];
  for (let i = 0; i < rows.length; i++) {
    if (rows[i].text.replace(/\s+$/, "") === want) candidates.push(i);
  }

  if (candidates.length === 0) {
    return { status: "lost", reason: "line text absent from new diff" };
  }

  if (candidates.length > 1) {
    const before = anchor.contextBefore;
    const after = anchor.contextAfter;
    if (before != null || after != null) {
      const filtered = candidates.filter((i) => {
        const b = i > 0 ? rows[i - 1].text : null;
        const a = i + 1 < rows.length ? rows[i + 1].text : null;
        if (before != null && b != null && trimEnd(b) !== trimEnd(before)) return false;
        if (after != null && a != null && trimEnd(a) !== trimEnd(after)) return false;
        return true;
      });
      if (filtered.length > 0) candidates = filtered;
    }
  }

  if (candidates.length > 1) {
    candidates.sort(
      (x, y) => Math.abs(rows[x].line - anchor.line) - Math.abs(rows[y].line - anchor.line),
    );
    const d0 = Math.abs(rows[candidates[0]].line - anchor.line);
    const d1 = Math.abs(rows[candidates[1]].line - anchor.line);
    if (d0 === d1) {
      return { status: "lost", reason: "ambiguous: multiple equal matches" };
    }
    candidates = candidates.slice(0, 1);
  }

  const row = rows[candidates[0]];
  return {
    status: "reanchored",
    line: row.line,
    hunk: row.hunk,
    moved: row.line !== anchor.line,
  };
}

function trimEnd(s: string): string {
  return s.replace(/\s+$/, "");
}
