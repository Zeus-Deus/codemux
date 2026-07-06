import { describe, it, expect } from "vitest";

import type { SubagentSnapshot } from "@/tauri/events";

import {
  countRunningSubagents,
  describeToolCall,
  findSubagentView,
  formatElapsed,
  mergeSnapshot,
  mergeStatus,
  newSubagentView,
  recentToolCalls,
  subagentActivityLine,
  subagentElapsedMs,
  subagentMetaLine,
  subagentOrdinal,
  subagentToolCount,
  toneIndexForId,
} from "./subagents";
import type {
  ChatViewItem,
  SubagentRunItem,
  SubagentView,
  ToolCallItem,
} from "./types";

function toolCall(overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call",
    id: `tc-${Math.random()}`,
    seq: 0,
    tool_use_id: "tu",
    tool_name: "Read",
    input: { file_path: "/f" },
    status: "done",
    result_content: null,
    approval_request_id: null,
    ...overrides,
  };
}

function view(overrides: Partial<SubagentView> = {}): SubagentView {
  return {
    id: "s1",
    status: "running",
    items: [],
    toneIndex: 0,
    ...overrides,
  };
}

describe("mergeStatus", () => {
  it("never regresses to pending", () => {
    expect(mergeStatus("running", "pending")).toBe("running");
    expect(mergeStatus("completed", "pending")).toBe("completed");
  });

  it("advances to running / terminal", () => {
    expect(mergeStatus("pending", "running")).toBe("running");
    expect(mergeStatus("running", "completed")).toBe("completed");
    expect(mergeStatus("pending", "failed")).toBe("failed");
  });

  it("does not downgrade a terminal state to running", () => {
    expect(mergeStatus("completed", "running")).toBe("completed");
  });
});

describe("mergeSnapshot", () => {
  it("merges only non-null fields (identity dribbled across events)", () => {
    let v = newSubagentView("s1", 1000);
    v = mergeSnapshot(v, {
      subagent_id: "s1",
      status: "running",
      name: "Explore",
    } as SubagentSnapshot);
    expect(v.name).toBe("Explore");
    expect(v.model).toBeUndefined();

    // A later snapshot adds the model without clobbering the name.
    v = mergeSnapshot(v, {
      subagent_id: "s1",
      status: "running",
      model: "opus",
    } as SubagentSnapshot);
    expect(v.name).toBe("Explore");
    expect(v.model).toBe("opus");
  });

  it("takes usage + result on completion and keeps status monotonic", () => {
    let v = view({ status: "running", name: "Impl" });
    v = mergeSnapshot(v, {
      subagent_id: "s1",
      status: "completed",
      result_text: "Done",
      tool_use_count: 28,
      duration_ms: 161000,
    } as SubagentSnapshot);
    expect(v.status).toBe("completed");
    expect(v.resultText).toBe("Done");
    expect(v.toolUseCount).toBe(28);
    expect(v.durationMs).toBe(161000);

    // A stray trailing pending update must not revive it.
    v = mergeSnapshot(v, {
      subagent_id: "s1",
      status: "pending",
    } as SubagentSnapshot);
    expect(v.status).toBe("completed");
  });
});

describe("derived meta / activity fallbacks", () => {
  it("tool-count falls back to counting child tool calls", () => {
    const v = view({ items: [toolCall(), toolCall(), { kind: "assistant_message", id: "a", seq: 1, turn_id: null, text: "x", streaming: false }] });
    expect(subagentToolCount(v)).toBe(2);
    // Provider usage wins when present.
    expect(subagentToolCount(view({ toolUseCount: 9, items: [toolCall()] }))).toBe(9);
  });

  it("elapsed falls back to now - startedAt when no duration", () => {
    expect(subagentElapsedMs(view({ startedAt: 1000 }), 3000)).toBe(2000);
    expect(subagentElapsedMs(view({ durationMs: 52000, startedAt: 1000 }), 9e9)).toBe(52000);
    expect(subagentElapsedMs(view({}), 5000)).toBeNull();
  });

  it("meta line combines elapsed + tool count", () => {
    const v = view({ durationMs: 161000, toolUseCount: 28 });
    expect(subagentMetaLine(v, 0)).toBe("2m 41s · 28 tools");
    expect(formatElapsed(52000)).toBe("0m 52s");
  });

  it("activity: provider summary wins, else latest tool as verb target", () => {
    expect(subagentActivityLine(view({ activity: "reading diff…" }))).toBe(
      "reading diff…",
    );
    const withTool = view({
      activity: undefined,
      items: [toolCall({ tool_name: "Bash", input: { command: "npm run verify" } })],
    });
    expect(subagentActivityLine(withTool)).toBe("run npm run verify");
  });

  it("done rows show the result first line, else 'Done'", () => {
    expect(
      subagentActivityLine(view({ status: "completed", resultText: "Line 1\nLine 2" })),
    ).toBe("Line 1");
    expect(subagentActivityLine(view({ status: "completed" }))).toBe("Done");
    expect(subagentActivityLine(view({ status: "failed" }))).toBe("Failed");
  });
});

describe("describeToolCall / peek", () => {
  it("maps verb + target + diff meta", () => {
    const d = describeToolCall(
      toolCall({
        tool_name: "Edit",
        input: { file_path: "a.ts", old_string: "x", new_string: "x\ny" },
      }),
    );
    expect(d.verb).toBe("edit");
    expect(d.target).toBe("a.ts");
    expect(d.meta).toBe("+2 −1");
  });

  it("running tools read as running in the peek", () => {
    const d = describeToolCall(
      toolCall({ tool_name: "Bash", status: "running", input: { command: "npm run verify" } }),
    );
    expect(d).toEqual({ verb: "run", target: "npm run verify", meta: "running" });
  });

  it("recentToolCalls returns up to N newest, in order", () => {
    const v = view({
      items: [
        toolCall({ id: "t1" }),
        { kind: "assistant_message", id: "a", seq: 1, turn_id: null, text: "x", streaming: false },
        toolCall({ id: "t2" }),
        toolCall({ id: "t3" }),
        toolCall({ id: "t4" }),
      ],
    });
    expect(recentToolCalls(v, 3).map((t) => t.id)).toEqual(["t2", "t3", "t4"]);
  });
});

describe("whole-thread lookups", () => {
  const card: SubagentRunItem = {
    kind: "subagent_run",
    id: "run-1",
    seq: 0,
    turn_id: "t1",
    subagents: [
      view({ id: "a", status: "running" }),
      view({ id: "b", status: "completed" }),
      view({ id: "c", status: "running" }),
    ],
  };
  const messages: ChatViewItem[] = [
    { kind: "user_message", id: "u", seq: 0, text: "hi" },
    card,
  ];

  it("counts running subagents across cards", () => {
    expect(countRunningSubagents(messages)).toBe(2);
  });

  it("finds a subagent view by id + its ordinal", () => {
    expect(findSubagentView(messages, "b")?.status).toBe("completed");
    expect(subagentOrdinal(messages, "c")).toBe(3);
    expect(findSubagentView(messages, "zzz")).toBeNull();
  });

  it("toneIndexForId is deterministic and bounded", () => {
    expect(toneIndexForId("abc")).toBe(toneIndexForId("abc"));
    expect(toneIndexForId("abc")).toBeGreaterThanOrEqual(0);
    expect(toneIndexForId("abc")).toBeLessThan(5);
  });
});
