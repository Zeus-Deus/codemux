export interface DiffLine {
  type: "add" | "del" | "context" | "hunk-header";
  content: string;
  oldLine: number | null;
  newLine: number | null;
}

export interface DiffHunk {
  header: string;
  lines: DiffLine[];
}

export interface SplitLine {
  left: DiffLine | null;
  right: DiffLine | null;
}

const METADATA_PREFIXES = [
  "diff ",
  "index ",
  "--- ",
  "+++ ",
  "new file",
  "deleted file",
  "old mode",
  "new mode",
  "similarity",
  "rename ",
  "\\ No newline",
];

/**
 * Parse unified diff text into structured lines.
 * Ported from src-old/components/diff/DiffContent.svelte.
 */
export function parseDiff(text: string): DiffLine[] {
  if (!text) return [];
  const lines: DiffLine[] = [];
  let oldLine = 0;
  let newLine = 0;

  for (const raw of text.split("\n")) {
    if (METADATA_PREFIXES.some((p) => raw.startsWith(p))) {
      continue;
    }

    if (raw.startsWith("@@")) {
      const match = raw.match(/@@ -(\d+)/);
      if (match) {
        oldLine = parseInt(match[1], 10);
        const newMatch = raw.match(/\+(\d+)/);
        newLine = newMatch ? parseInt(newMatch[1], 10) : oldLine;
      }
      lines.push({
        type: "hunk-header",
        content: raw,
        oldLine: null,
        newLine: null,
      });
    } else if (raw.startsWith("+")) {
      lines.push({ type: "add", content: raw.slice(1), oldLine: null, newLine });
      newLine++;
    } else if (raw.startsWith("-")) {
      lines.push({ type: "del", content: raw.slice(1), oldLine, newLine: null });
      oldLine++;
    } else {
      const content = raw.startsWith(" ") ? raw.slice(1) : raw;
      if (content || lines.length > 0) {
        lines.push({ type: "context", content, oldLine, newLine });
        oldLine++;
        newLine++;
      }
    }
  }
  return lines;
}

/** Group parsed diff lines into hunks for navigation. */
export function groupHunks(lines: DiffLine[]): DiffHunk[] {
  const hunks: DiffHunk[] = [];
  let current: DiffHunk | null = null;

  for (const line of lines) {
    if (line.type === "hunk-header") {
      current = { header: line.content, lines: [line] };
      hunks.push(current);
    } else if (current) {
      current.lines.push(line);
    } else {
      // Lines before first hunk header (shouldn't normally happen)
      current = { header: "", lines: [line] };
      hunks.push(current);
    }
  }
  return hunks;
}

/**
 * Build side-by-side line pairs for split view.
 * Pairs deletions (left) with additions (right) in change blocks.
 * Context lines appear on both sides. Unpaired lines get null on the other side.
 */
export function buildSplitPairs(lines: DiffLine[]): SplitLine[] {
  const pairs: SplitLine[] = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];

    if (line.type === "hunk-header") {
      pairs.push({ left: line, right: line });
      i++;
      continue;
    }

    if (line.type === "context") {
      pairs.push({ left: line, right: line });
      i++;
      continue;
    }

    // Collect consecutive del/add block
    const dels: DiffLine[] = [];
    const adds: DiffLine[] = [];

    while (i < lines.length && lines[i].type === "del") {
      dels.push(lines[i]);
      i++;
    }
    while (i < lines.length && lines[i].type === "add") {
      adds.push(lines[i]);
      i++;
    }

    const maxLen = Math.max(dels.length, adds.length);
    for (let j = 0; j < maxLen; j++) {
      pairs.push({
        left: j < dels.length ? dels[j] : null,
        right: j < adds.length ? adds[j] : null,
      });
    }
  }

  return pairs;
}
