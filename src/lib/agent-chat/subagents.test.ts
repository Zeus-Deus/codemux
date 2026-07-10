import { describe, it, expect } from "vitest";

import type { SubagentSnapshot } from "@/tauri/events";

import {
  countRunningSubagents,
  describeToolCall,
  findSubagentView,
  formatElapsed,
  interruptRunningSubagents,
  isDone,
  mergeSnapshot,
  mergeStatus,
  newSubagentView,
  recentToolCalls,
  runningSubagentEntries,
  settleSubagentsForToolResult,
  subagentActivityLine,
  subagentElapsedMs,
  subagentMetaLine,
  subagentOrdinal,
  subagentStatusLabel,
  subagentToolCount,
  toneIndexForId,
} from "./subagents";
import type {
  ChatViewItem,
  SubagentRunItem,
  SubagentView,
  WorkflowRunItem,
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

  it("does not let a stray running snapshot regress interrupted (revive is mergeSnapshot's job)", () => {
    expect(mergeStatus("interrupted", "running")).toBe("interrupted");
    expect(mergeStatus("interrupted", "completed")).toBe("completed");
  });

  it("treats a missing/unknown incoming status as pending (no-op) instead of leaking it into the view", () => {
    // The wire type says `status` is required, but replayed dev fixtures /
    // hand-built payloads can omit it (Rust serde-defaults to `pending`).
    // An `undefined` leaking through used to crash `statusTone` at render.
    expect(
      mergeStatus("running", undefined as unknown as SubagentSnapshot["status"]),
    ).toBe("running");
    expect(
      mergeStatus("completed", "bogus" as SubagentSnapshot["status"]),
    ).toBe("completed");
  });
});

describe("mergeSnapshot revive / statusAssumed (issue #153)", () => {
  const snap = (status: SubagentSnapshot["status"]): SubagentSnapshot =>
    ({ subagent_id: "s1", status }) as SubagentSnapshot;

  it("a real running snapshot revives an interrupted row and clears assumed", () => {
    const v = view({ status: "interrupted", statusAssumed: true });
    const next = mergeSnapshot(v, snap("running"));
    expect(next.status).toBe("running");
    expect(next.statusAssumed).toBe(false);
  });

  it("a real running snapshot revives an ASSUMED-completed row (Claude background task tick)", () => {
    const v = view({ status: "completed", statusAssumed: true });
    const next = mergeSnapshot(v, snap("running"));
    expect(next.status).toBe("running");
    expect(next.statusAssumed).toBe(false);
  });

  it("a real terminal snapshot wins over an assumed one and clears assumed", () => {
    const v = view({ status: "completed", statusAssumed: true });
    const next = mergeSnapshot(v, snap("failed"));
    expect(next.status).toBe("failed");
    expect(next.statusAssumed).toBe(false);
  });

  it("a real completed snapshot settles an interrupted row (not revived by a terminal)", () => {
    const v = view({ status: "interrupted", statusAssumed: true });
    const next = mergeSnapshot(v, snap("completed"));
    expect(next.status).toBe("completed");
    expect(next.statusAssumed).toBe(false);
  });

  it("a status-less snapshot (activity-only tick) leaves the status untouched", () => {
    const v = view({ status: "running" });
    const next = mergeSnapshot(v, {
      subagent_id: "s1",
      activity: "mapping the call graph…",
    } as unknown as SubagentSnapshot);
    expect(next.status).toBe("running");
    expect(next.activity).toBe("mapping the call graph…");
  });

  it("maps parent_item_id → parentItemId", () => {
    const v = newSubagentView("s1", 0);
    const next = mergeSnapshot(v, {
      subagent_id: "s1",
      status: "running",
      parent_item_id: "tool-use-9",
    } as SubagentSnapshot);
    expect(next.parentItemId).toBe("tool-use-9");
  });
});

describe("settleSubagentsForToolResult / interruptRunningSubagents", () => {
  function card(subs: SubagentView[]): SubagentRunItem {
    return { kind: "subagent_run", id: "run-1", seq: 0, turn_id: "t", subagents: subs };
  }
  function workflowWith(agents: SubagentView[]): WorkflowRunItem {
    return {
      kind: "workflow_run",
      id: "wf-1",
      seq: 1,
      workflowId: "wf",
      status: "running",
      name: null,
      description: null,
      script: null,
      plannedPhases: [{ title: "P", detail: null }],
      phases: [{ title: "P", detail: null, agents }],
      resultText: null,
      totalTokens: null,
      agentCount: null,
      startedAt: 0,
      durationMs: null,
      approvalRequestId: null,
    };
  }

  it("settles a running subagent matched by id, and by parentItemId", () => {
    const messages: ChatViewItem[] = [
      card([
        view({ id: "tool-a", status: "running" }),
        view({ id: "child", status: "running", parentItemId: "tool-b" }),
        view({ id: "other", status: "running" }),
      ]),
    ];
    const byId = settleSubagentsForToolResult(messages, "tool-a", false);
    const c1 = byId[0] as SubagentRunItem;
    expect(c1.subagents[0].status).toBe("completed");
    expect(c1.subagents[0].statusAssumed).toBe(true);
    expect(c1.subagents[2].status).toBe("running"); // untouched

    const byParent = settleSubagentsForToolResult(messages, "tool-b", false);
    expect((byParent[0] as SubagentRunItem).subagents[1].status).toBe("completed");
  });

  it("uses failed when the tool_result is an error", () => {
    const messages: ChatViewItem[] = [card([view({ id: "tool-a", status: "running" })])];
    const out = settleSubagentsForToolResult(messages, "tool-a", true);
    expect((out[0] as SubagentRunItem).subagents[0].status).toBe("failed");
  });

  it("returns the SAME array reference when nothing matched or is already terminal", () => {
    const done = view({ id: "tool-a", status: "completed" });
    const messages: ChatViewItem[] = [card([done])];
    // Already terminal → untouched, same ref.
    expect(settleSubagentsForToolResult(messages, "tool-a", false)).toBe(messages);
    // No id/parent match → same ref.
    expect(settleSubagentsForToolResult(messages, "nope", false)).toBe(messages);
  });

  it("settles running agents inside workflow phases too", () => {
    const messages: ChatViewItem[] = [
      workflowWith([view({ id: "wa", status: "running", parentItemId: "wf" })]),
    ];
    const out = settleSubagentsForToolResult(messages, "wf", false);
    const wf = out[0] as WorkflowRunItem;
    expect(wf.phases[0].agents[0].status).toBe("completed");
    expect(wf.phases[0].agents[0].statusAssumed).toBe(true);
  });

  it("interruptRunningSubagents flips running/pending to interrupted across cards and phases", () => {
    const messages: ChatViewItem[] = [
      card([view({ id: "a", status: "running" }), view({ id: "b", status: "completed" })]),
      workflowWith([view({ id: "c", status: "pending" })]),
    ];
    const out = interruptRunningSubagents(messages);
    expect((out[0] as SubagentRunItem).subagents[0].status).toBe("interrupted");
    expect((out[0] as SubagentRunItem).subagents[0].statusAssumed).toBe(true);
    expect((out[0] as SubagentRunItem).subagents[1].status).toBe("completed"); // untouched
    expect((out[1] as WorkflowRunItem).phases[0].agents[0].status).toBe("interrupted");
  });

  it("interruptRunningSubagents returns the SAME ref when nothing is running", () => {
    const messages: ChatViewItem[] = [card([view({ id: "a", status: "completed" })])];
    expect(interruptRunningSubagents(messages)).toBe(messages);
  });
});

describe("interrupted view status", () => {
  it("is done, not running, and labelled/activity-lined", () => {
    const v = view({ status: "interrupted" });
    expect(isDone(v)).toBe(true);
    expect(subagentStatusLabel(v)).toBe("Interrupted");
    expect(subagentActivityLine(v)).toBe("Interrupted");
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

  it("flattens running subagents with no from-label when there is one card", () => {
    const entries = runningSubagentEntries(messages);
    expect(entries.map((e) => e.subagent.id)).toEqual(["a", "c"]);
    expect(entries.every((e) => e.cardId === "run-1")).toBe(true);
    expect(entries.every((e) => e.fromLabel === null)).toBe(true);
  });

  it("labels each running subagent with its originating card once several cards exist", () => {
    const cardTwo: SubagentRunItem = {
      kind: "subagent_run",
      id: "run-2",
      seq: 1,
      turn_id: "t2",
      subagents: [view({ id: "d", status: "running" })],
    };
    const multi: ChatViewItem[] = [...messages, cardTwo];
    const entries = runningSubagentEntries(multi);
    expect(entries.map((e) => [e.subagent.id, e.cardId, e.fromLabel])).toEqual([
      ["a", "run-1", "task 1"],
      ["c", "run-1", "task 1"],
      ["d", "run-2", "task 2"],
    ]);
  });
});
