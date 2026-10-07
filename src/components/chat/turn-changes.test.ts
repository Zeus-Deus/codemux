import { describe, expect, it } from "vitest";

import type { ChatViewItem, ToolCallItem } from "@/lib/agent-chat/types";

import { summarizeTurnChanges, turnChangesEqual } from "./turn-changes";

let seq = 0;
function tool(
  tool_name: string,
  input: unknown,
  status: ToolCallItem["status"] = "done",
): ToolCallItem {
  seq += 1;
  return {
    kind: "tool_call",
    id: `tool-${seq}`,
    seq,
    tool_use_id: `use-${seq}`,
    tool_name,
    input,
    status,
    result_content: null,
    approval_request_id: null,
  };
}

describe("summarizeTurnChanges", () => {
  it("sums edit counts per file across a turn's edit calls", () => {
    const items: ChatViewItem[] = [
      tool("Read", { file_path: "/repo/src/a.ts" }),
      tool("Edit", {
        file_path: "/repo/src/a.ts",
        old_string: "one\ntwo",
        new_string: "one\n2\nthree",
      }),
      tool("Write", { file_path: "/repo/src/b.ts", content: "x\ny" }),
      tool("Edit", {
        file_path: "/repo/src/a.ts",
        old_string: "three",
        new_string: "3",
      }),
    ];
    expect(summarizeTurnChanges(items)).toEqual({
      files: [
        { path: "/repo/src/a.ts", added: 3, removed: 2 },
        { path: "/repo/src/b.ts", added: 2, removed: 0 },
      ],
      added: 5,
      removed: 2,
    });
  });

  it("leaves out failed and in-flight edits, and read-only turns", () => {
    expect(
      summarizeTurnChanges([
        tool("Edit", { file_path: "/a", old_string: "a", new_string: "b" }, "error"),
        tool("Edit", { file_path: "/a", old_string: "a", new_string: "b" }, "running"),
        tool("Grep", { pattern: "x" }),
      ]),
    ).toBeNull();
  });

  it("reads Codex file changes", () => {
    const summary = summarizeTurnChanges([
      tool("fileChange", {
        type: "fileChange",
        changes: [
          {
            path: "/repo/new.ts",
            kind: { type: "add" },
            diff: "line 1\nline 2\n",
          },
          {
            path: "/repo/old.ts",
            kind: { type: "delete" },
            diff: "gone\n",
          },
          {
            path: "/repo/main.ts",
            kind: { type: "update", move_path: null },
            diff: "--- a/main.ts\n+++ b/main.ts\n@@ -1,2 +1,2 @@\n-old\n+new\n same\n",
          },
        ],
      }),
    ]);
    expect(summary?.files).toEqual([
      { path: "/repo/new.ts", added: 2, removed: 0 },
      { path: "/repo/old.ts", added: 0, removed: 1 },
      { path: "/repo/main.ts", added: 1, removed: 1 },
    ]);
  });

  it("compares summaries by value so unchanged folds keep their slot", () => {
    const edit = () =>
      tool("Edit", { file_path: "/a", old_string: "a", new_string: "b" });
    const a = summarizeTurnChanges([edit()]) ?? undefined;
    const b = summarizeTurnChanges([edit()]) ?? undefined;
    expect(a).not.toBe(b);
    expect(turnChangesEqual(a, b)).toBe(true);
    expect(turnChangesEqual(a, undefined)).toBe(false);
  });
});
