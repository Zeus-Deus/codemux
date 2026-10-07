import type { ChatViewItem, ToolCallItem } from "@/lib/agent-chat/types";
import { parseDiff } from "@/lib/diff-parser";

import { editCounts, type EditCounts } from "./activity-steps";

/** One file a settled turn wrote to, with its summed line counts. */
export interface TurnFileChange {
  path: string;
  added: number;
  removed: number;
}

/** What a turn touched, answered from its own edit tool calls. */
export interface TurnChangeSummary {
  /** In first-touched order. */
  files: TurnFileChange[];
  added: number;
  removed: number;
}

interface FileEdit extends EditCounts {
  path: string;
}

const EDIT_TOOLS = new Set(["Edit", "MultiEdit", "Write", "NotebookEdit"]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stringField(record: Record<string, unknown>, key: string): string | null {
  const value = record[key];
  return typeof value === "string" && value.length > 0 ? value : null;
}

function lineCount(text: string): number {
  if (text.length === 0) return 0;
  const lines = text.split("\n");
  return lines[lines.length - 1] === "" ? lines.length - 1 : lines.length;
}

/** Codex reports a file change as `{ path, kind: { type }, diff }`, where an
 *  added or deleted file carries its whole content and an update carries a
 *  unified diff. */
function codexFileEdits(input: Record<string, unknown>): FileEdit[] {
  const changes = Array.isArray(input.changes) ? input.changes : [];
  const edits: FileEdit[] = [];
  for (const change of changes) {
    if (!isRecord(change)) continue;
    const path = stringField(change, "path");
    if (!path) continue;
    const diff = typeof change.diff === "string" ? change.diff : "";
    const kind = isRecord(change.kind) ? stringField(change.kind, "type") : null;
    if (kind === "add") {
      edits.push({ path, added: lineCount(diff), removed: 0 });
      continue;
    }
    if (kind === "delete") {
      edits.push({ path, added: 0, removed: lineCount(diff) });
      continue;
    }
    // The shared parser only treats `---`/`+++` as file headers before the
    // first hunk; inside one they are content (a deleted `-- note` line).
    const rows = parseDiff(diff);
    edits.push({
      path,
      added: rows.filter((row) => row.type === "add").length,
      removed: rows.filter((row) => row.type === "del").length,
    });
  }
  return edits;
}

// Slots are rebuilt on every streamed token, and the line diff behind an
// Edit's counts is an LCS. The reducer keeps a settled call's object stable,
// so caching by identity makes re-summarising old turns free.
const editsCache = new WeakMap<ToolCallItem, FileEdit[]>();

function fileEdits(item: ToolCallItem): FileEdit[] {
  const cached = editsCache.get(item);
  if (cached) return cached;
  let edits: FileEdit[] = [];
  const input = isRecord(item.input) ? item.input : null;
  if (input && item.tool_name === "fileChange") {
    edits = codexFileEdits(input);
  } else if (input && EDIT_TOOLS.has(item.tool_name)) {
    const path =
      stringField(input, "file_path") ??
      stringField(input, "path") ??
      stringField(input, "notebook_path");
    if (path) {
      const counts = editCounts(item) ?? { added: 0, removed: 0 };
      edits = [{ path, added: counts.added, removed: counts.removed }];
    }
  }
  editsCache.set(item, edits);
  return edits;
}

/** Sum a turn's successful file edits per path. `null` when the turn wrote
 *  nothing, so a read-only turn shows no change chip. Failed calls are left
 *  out because they did not touch the file. */
export function summarizeTurnChanges(
  items: readonly ChatViewItem[],
): TurnChangeSummary | null {
  const byPath = new Map<string, TurnFileChange>();
  for (const item of items) {
    if (item.kind !== "tool_call" || item.status !== "done") continue;
    for (const edit of fileEdits(item)) {
      const existing = byPath.get(edit.path);
      if (existing) {
        existing.added += edit.added;
        existing.removed += edit.removed;
      } else {
        byPath.set(edit.path, { ...edit });
      }
    }
  }
  if (byPath.size === 0) return null;
  const files = [...byPath.values()];
  return {
    files,
    added: files.reduce((sum, file) => sum + file.added, 0),
    removed: files.reduce((sum, file) => sum + file.removed, 0),
  };
}

export function turnChangesEqual(
  a: TurnChangeSummary | undefined,
  b: TurnChangeSummary | undefined,
): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return (
    a.added === b.added &&
    a.removed === b.removed &&
    a.files.length === b.files.length &&
    a.files.every((file, index) => {
      const other = b.files[index];
      return (
        file.path === other.path &&
        file.added === other.added &&
        file.removed === other.removed
      );
    })
  );
}
