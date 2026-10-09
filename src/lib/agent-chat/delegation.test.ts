import { describe, expect, it } from "vitest";

import {
  DELEGATION_RESULTS_PREFIX,
  childThreadIdOf,
  delegateCallInput,
  delegatedSubagentId,
  delegationPhaseSummary,
  delegationRow,
  formatDelegationElapsed,
  isDelegatedRow,
  isDelegateToolCall,
  isDelegateToolName,
  isDelegationResultsText,
  linkDelegations,
  modelEffortLabel,
  parseDelegateResult,
  parseWake,
  providerKindOf,
  wakeSummary,
} from "./delegation";
import { LAZY_TOOL_RESULT_KEY } from "./lazy-tool-result";
import type { ChatViewItem, SubagentRunItem, SubagentView, ToolCallItem } from "./types";

const STARTED = {
  started: {
    title: "Add slugify helper",
    provider: "codex",
    model: "gpt-6.1-codex",
    effort: "high",
    thread: "child-1",
  },
  note: "Codex is working in a background tab.",
};

const INPUT = {
  provider: "codex",
  title: "Add slugify helper",
  task: "Add slugify(text) to src/lib/strings.ts with table tests.",
  model: "gpt-6.1-codex",
  effort: "high",
};

function call(overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call",
    id: "tool-1",
    seq: 1,
    turn_id: "t1",
    tool_use_id: "toolu_1",
    tool_name: "mcp__codemux__delegate_task",
    input: INPUT,
    status: "done",
    result_content: [{ type: "text", text: JSON.stringify(STARTED) }],
    approval_request_id: null,
    ...overrides,
  };
}

function view(overrides: Partial<SubagentView> = {}): SubagentView {
  return {
    id: delegatedSubagentId("child-1"),
    name: "Codex",
    agentType: "codex",
    description: "Add slugify helper",
    model: "gpt-6.1-codex",
    effort: "high",
    status: "running",
    activity: "Working in its tab",
    items: [],
    toneIndex: 0,
    ...overrides,
  };
}

function card(id: string, seq: number, subagents: SubagentView[]): SubagentRunItem {
  return { kind: "subagent_run", id, seq, turn_id: "t1", subagents };
}

describe("delegated row identity", () => {
  it("marks rows by the delegate: id prefix, on views and wire snapshots", () => {
    expect(isDelegatedRow({ id: "delegate:abc" })).toBe(true);
    expect(isDelegatedRow({ subagent_id: "delegate:abc" })).toBe(true);
    expect(isDelegatedRow({ id: "toolu_native" })).toBe(false);
  });

  it("decodes the child thread id", () => {
    expect(childThreadIdOf("delegate:0f6c2a9e")).toBe("0f6c2a9e");
    expect(childThreadIdOf(delegatedSubagentId("x"))).toBe("x");
    expect(childThreadIdOf("delegate:")).toBeNull();
    expect(childThreadIdOf("explore")).toBeNull();
  });

  it("normalises provider kinds and rejects unknown ones", () => {
    expect(providerKindOf("Codex")).toBe("codex");
    expect(providerKindOf(" claude ")).toBe("claude");
    expect(providerKindOf("gemini")).toBeNull();
    expect(providerKindOf(undefined)).toBeNull();
  });
});

describe("delegate_task call input", () => {
  it("matches any adapter prefix ending in delegate_task", () => {
    expect(isDelegateToolName("mcp__codemux__delegate_task")).toBe(true);
    expect(isDelegateToolName("delegate_task")).toBe(true);
    expect(isDelegateToolName("delegate_tasks")).toBe(false);
  });

  it("reads Claude's flat input", () => {
    expect(delegateCallInput("mcp__codemux__delegate_task", INPUT)).toEqual(INPUT);
  });

  it("reads Codex's dynamicToolCall envelope, with object or string arguments", () => {
    const envelope = { type: "dynamicToolCall", tool: "codemux_mcp__codemux__delegate_task" };
    expect(
      delegateCallInput("dynamicToolCall", { ...envelope, arguments: INPUT }),
    ).toEqual(INPUT);
    expect(
      delegateCallInput("dynamicToolCall", {
        ...envelope,
        arguments: JSON.stringify(INPUT),
      }),
    ).toEqual(INPUT);
    expect(
      delegateCallInput("dynamicToolCall", { tool: "codemux_mcp__git_status", arguments: {} }),
    ).toBeNull();
  });

  it("requires provider and task, and nulls the optional fields", () => {
    expect(delegateCallInput("delegate_task", { provider: "codex" })).toBeNull();
    expect(delegateCallInput("delegate_task", { task: "do it properly please" })).toBeNull();
    expect(
      delegateCallInput("delegate_task", { provider: "claude", task: "Review the parser." }),
    ).toEqual({
      provider: "claude",
      task: "Review the parser.",
      title: null,
      model: null,
      effort: null,
    });
  });

  it("recognises delegate calls among tool calls", () => {
    expect(isDelegateToolCall(call())).toBe(true);
    expect(isDelegateToolCall(call({ tool_name: "Bash", input: { command: "ls" } }))).toBe(false);
  });
});

describe("parseDelegateResult", () => {
  const started = { kind: "started", thread: "child-1" };

  it("parses the started JSON from every content shape", () => {
    const text = JSON.stringify(STARTED);
    expect(parseDelegateResult(text)).toEqual(started);
    expect(parseDelegateResult([{ type: "text", text }])).toEqual(started);
    expect(parseDelegateResult({ content: [{ type: "text", text }] })).toEqual(started);
    expect(
      parseDelegateResult({
        type: "dynamicToolCall",
        success: true,
        contentItems: [{ type: "inputText", text }],
      }),
    ).toEqual(started);
    expect(parseDelegateResult(STARTED)).toEqual(started);
  });

  it("reads a lazy stub's preview", () => {
    const stub = {
      [LAZY_TOOL_RESULT_KEY]: {
        row_id: 7,
        bytes: 40_000,
        preview: JSON.stringify(STARTED),
        line_count: 1,
        has_images: false,
      },
    };
    expect(parseDelegateResult(stub)).toEqual(started);
  });

  it("reads the thread whatever else the started result carries", () => {
    const text = JSON.stringify({
      started: { title: "T", provider: "claude", model: null, effort: null, thread: "c2" },
    });
    expect(parseDelegateResult(text)).toEqual({ kind: "started", thread: "c2" });
  });

  it("returns the refusal sentence for error results", () => {
    const sentence =
      "Delegation needs this chat in Full access; it is in plan. Switch to Full access to delegate.";
    expect(parseDelegateResult([{ type: "text", text: sentence }], true)).toEqual({
      kind: "error",
      message: sentence,
    });
    expect(
      parseDelegateResult({
        success: false,
        contentItems: [{ type: "inputText", text: "codex is not installed on this machine." }],
      }),
    ).toEqual({ kind: "error", message: "codex is not installed on this machine." });
  });

  it("returns null while there is no result or it is unreadable", () => {
    expect(parseDelegateResult(null)).toBeNull();
    expect(parseDelegateResult("not json")).toBeNull();
    expect(parseDelegateResult({ unexpected: true })).toBeNull();
  });
});

describe("delegationRow", () => {
  it("shows a running call with no row yet as Starting, with no stop target", () => {
    const row = delegationRow(call({ status: "running", result_content: null }), null);
    expect(row).toMatchObject({
      provider: "codex",
      providerLabel: "Codex",
      title: "Add slugify helper",
      model: "gpt-6.1-codex",
      effort: "high",
      phase: "starting",
      line: "Starting…",
      childThreadId: null,
      live: false,
    });
  });

  it("knows the child once the started result is in", () => {
    const row = delegationRow(call(), null);
    expect(row.phase).toBe("starting");
    expect(row.childThreadId).toBe("child-1");
    expect(row.live).toBe(true);
  });

  it("turns a refused call into Failed with the reason", () => {
    const row = delegationRow(
      call({
        status: "error",
        result_content: [
          { type: "text", text: "This chat already has 3 delegated tasks running. Wait for their reports." },
        ],
      }),
      null,
    );
    expect(row.phase).toBe("failed");
    expect(row.line).toBe("This chat already has 3 delegated tasks running. Wait for their reports.");
    expect(row.live).toBe(false);
  });

  it("maps the row's status and activity to a phase", () => {
    expect(delegationRow(null, view({ status: "pending", activity: "Starting…" })).phase).toBe(
      "starting",
    );
    expect(delegationRow(null, view()).phase).toBe("working");
    expect(
      delegationRow(null, view({ activity: "Waiting for your answer in its tab" })).phase,
    ).toBe("waiting");
    expect(
      delegationRow(null, view({ activity: "Paused: Codex usage limit · resumes 3:40 PM" }))
        .phase,
    ).toBe("paused");
    expect(delegationRow(null, view({ status: "stopped" })).phase).toBe("stopped");
    expect(delegationRow(null, view({ status: "failed", resultText: "usage limit reached" })))
      .toMatchObject({ phase: "failed", line: "usage limit reached", live: false });
  });

  it("flattens a completed report into one plain excerpt line", () => {
    const row = delegationRow(
      call(),
      view({
        status: "completed",
        resultText: "Added `slugify` to src/lib/strings.ts.\n\n- Checked: **9 passed**.",
      }),
    );
    expect(row.phase).toBe("done");
    expect(row.line).toBe("Added slugify to src/lib/strings.ts. Checked: 9 passed.");
  });

  it("lets the live row win over the call's arguments", () => {
    const row = delegationRow(
      call(),
      view({ description: "Trimmed title", model: "gpt-6.2", name: "Codex CLI" }),
    );
    expect(row.title).toBe("Trimmed title");
    expect(row.model).toBe("gpt-6.2");
    expect(row.providerLabel).toBe("Codex CLI");
  });

  it("titles an untitled call from its brief", () => {
    const row = delegationRow(
      call({
        input: { provider: "claude", task: "Review the parser for off-by-one bugs." },
        status: "running",
        result_content: null,
      }),
      null,
    );
    expect(row.title).toBe("Review the parser for off-by-one bugs.");
    expect(row.providerLabel).toBe("Claude");
  });

  it("labels model and effort together, either alone, or not at all", () => {
    expect(modelEffortLabel({ model: "gpt-6.1-codex", effort: "high" })).toBe(
      "gpt-6.1-codex · high",
    );
    expect(modelEffortLabel({ model: null, effort: "max" })).toBe("max");
    expect(modelEffortLabel({ model: null, effort: null })).toBeNull();
  });

  it("summarises a group's phases in order, counting a start as working", () => {
    const rows = [
      delegationRow(null, view({ id: "delegate:c3", status: "completed" })),
      delegationRow(null, view()),
      delegationRow(null, view({ id: "delegate:c2", activity: "Waiting for your answer in its tab" })),
      delegationRow(null, view({ id: "delegate:c4", status: "pending" })),
    ];
    expect(delegationPhaseSummary(rows)).toBe("2 working · 1 waiting for you · 1 done");
    expect(delegationPhaseSummary(rows.slice(1, 2))).toBe("1 working");
  });

  it("formats elapsed time compactly", () => {
    expect(formatDelegationElapsed(4_200)).toBe("4s");
    expect(formatDelegationElapsed(134_000)).toBe("2m 14s");
    expect(formatDelegationElapsed(3_900_000)).toBe("1h 05m");
  });
});

describe("linkDelegations", () => {
  it("pairs a call with the row its result names", () => {
    const messages: ChatViewItem[] = [call(), card("sub-1", 2, [view()])];
    const links = linkDelegations(messages);
    expect(links.viewByCallId.get("tool-1")?.id).toBe("delegate:child-1");
    expect(links.claimed.has("delegate:child-1")).toBe(true);
  });

  it("lets a still-running call claim the first unclaimed row after it", () => {
    const messages: ChatViewItem[] = [
      call({ status: "running", result_content: null }),
      card("sub-1", 2, [view()]),
    ];
    expect(linkDelegations(messages).viewByCallId.get("tool-1")?.id).toBe("delegate:child-1");
  });

  it("leaves rows with no call unclaimed", () => {
    const messages: ChatViewItem[] = [card("sub-1", 2, [view({ id: "delegate:orphan" })])];
    const links = linkDelegations(messages);
    expect(links.viewByCallId.size).toBe(0);
    expect(links.claimed.size).toBe(0);
  });

  it("never lets a refused call take a sibling's row", () => {
    const messages: ChatViewItem[] = [
      call({
        id: "refused",
        status: "error",
        result_content: [{ type: "text", text: "Delegation is off." }],
      }),
      call({ id: "starting", seq: 2, status: "running", result_content: null }),
      card("sub-1", 3, [view({ status: "pending" })]),
    ];
    const links = linkDelegations(messages);
    expect(links.viewByCallId.has("refused")).toBe(false);
    expect(links.viewByCallId.get("starting")?.id).toBe("delegate:child-1");
  });

  it("claims a row that landed just before the call (Codex), within the same turn", () => {
    const raced: ChatViewItem[] = [
      card("sub-1", 1, [view({ status: "pending" })]),
      call({ seq: 2, status: "running", result_content: null }),
    ];
    expect(linkDelegations(raced).viewByCallId.get("tool-1")?.id).toBe("delegate:child-1");

    const earlierTurn: ChatViewItem[] = [
      card("sub-1", 1, [view()]),
      { kind: "user_message", id: "um-2", seq: 2, text: "next", created_at: 0 },
      call({ seq: 3, status: "running", result_content: null }),
    ];
    expect(linkDelegations(earlierTurn).claimed.size).toBe(0);
  });

  it("pairs in-flight calls with rows in hand-off order, even when rows race ahead", () => {
    const review = { ...INPUT, title: "Review the parser" };
    // Both rows landed before their calls: the backend handles calls in order.
    const messages: ChatViewItem[] = [
      card("sub-1", 1, [view({ status: "pending" })]),
      call({ id: "slugify", seq: 2, status: "running", result_content: null }),
      card("sub-2", 3, [
        view({ id: "delegate:child-2", description: "Review the parser", status: "pending" }),
      ]),
      call({ id: "review", seq: 4, input: review, status: "running", result_content: null }),
    ];
    const links = linkDelegations(messages);
    expect(links.viewByCallId.get("slugify")?.id).toBe("delegate:child-1");
    expect(links.viewByCallId.get("review")?.id).toBe("delegate:child-2");
  });

  it("lets an in-flight call take only a row of its own provider", () => {
    const messages: ChatViewItem[] = [
      call({ status: "running", result_content: null }),
      card("sub-1", 2, [view({ id: "delegate:claude-1", agentType: "claude", status: "pending" })]),
    ];
    expect(linkDelegations(messages).claimed.size).toBe(0);
  });

  it("links by the result's thread once it lands, whatever the order", () => {
    const started = (thread: string) => [
      { type: "text", text: JSON.stringify({ started: { thread } }) },
    ];
    const messages: ChatViewItem[] = [
      call({ id: "first", result_content: started("child-2") }),
      call({ id: "second", seq: 2, result_content: started("child-1") }),
      card("sub-1", 3, [view(), view({ id: "delegate:child-2" })]),
    ];
    const links = linkDelegations(messages);
    expect(links.viewByCallId.get("first")?.id).toBe("delegate:child-2");
    expect(links.viewByCallId.get("second")?.id).toBe("delegate:child-1");
  });
});

describe("the results turn", () => {
  const wake = [
    DELEGATION_RESULTS_PREFIX,
    "Codemux posted this message, not the user.",
    "",
    '<task provider="codex" model="gpt-6.1-codex" effort="high" status="completed" title="Add &quot;slug&quot; helper" duration="6m 12s" thread="c1">',
    "<brief>Add slugify.</brief>",
    "<report>\nDone.\n</report>",
    "</task>",
    "",
    '<task provider="claude" status="failed" title="Review the parser" duration="12s" thread="c2">',
    "<report>Claude is not signed in.</report>",
    "</task>",
    "",
    '<task provider="cursor" status="stopped" title="Write parser tests" duration="1m 03s" thread="c3">',
    "The user stopped this task.",
    "</task>",
  ].join("\n");

  it("detects the prefix", () => {
    expect(isDelegationResultsText(wake)).toBe(true);
    expect(isDelegationResultsText(`hello\n${DELEGATION_RESULTS_PREFIX}`)).toBe(false);
  });

  it("parses each task block's attributes", () => {
    expect(parseWake(wake)).toEqual([
      {
        provider: "codex",
        status: "completed",
        title: 'Add "slug" helper',
        model: "gpt-6.1-codex",
        effort: "high",
        thread: "c1",
      },
      {
        provider: "claude",
        status: "failed",
        title: "Review the parser",
        model: null,
        effort: null,
        thread: "c2",
      },
      {
        provider: "cursor",
        status: "stopped",
        title: "Write parser tests",
        model: null,
        effort: null,
        thread: "c3",
      },
    ]);
  });

  it("never counts a task tag quoted inside a brief or report", () => {
    const quoted = '<task provider="claude" status="failed" title="x">';
    const text = [
      DELEGATION_RESULTS_PREFIX,
      "Codemux posted this message, not the user.",
      "",
      '<task provider="codex" model="default" status="completed" title="Review the wake" duration="1m 02s" thread="c1">',
      `<brief>Check that each block opens with a line like\n${quoted}\nand wraps text in <report> tags.</brief>`,
      "<report>",
      "Blocks look like this:",
      quoted,
      "<brief>see above",
      "</report> appears once per block.",
      "</report>",
      "</task>",
      "",
      "No delegated tasks are still running.",
    ].join("\n");
    const tasks = parseWake(text);
    expect(tasks.map((task) => [task.provider, task.status])).toEqual([["codex", "completed"]]);
    expect(wakeSummary(tasks).label).toBe("Delegated results · Codex · completed");
  });

  it("ignores text without the prefix and blocks without a provider", () => {
    expect(parseWake('<task provider="codex" status="completed">')).toEqual([]);
    expect(parseWake(`${DELEGATION_RESULTS_PREFIX}\n<task status="completed" title="x">`)).toEqual(
      [],
    );
  });

  it("summarises providers and outcome for the divider", () => {
    expect(wakeSummary(parseWake(wake))).toEqual({
      providers: ["codex", "claude", "cursor"],
      names: "Codex, Claude, Cursor",
      outcome: "1 completed · 1 failed · 1 stopped",
      label: "Delegated results · Codex, Claude, Cursor · 1 completed · 1 failed · 1 stopped",
      failed: true,
    });
    const allDone = wakeSummary([
      { provider: "codex", status: "completed", title: "a", model: null, effort: null, thread: null },
      { provider: "codex", status: "completed", title: "b", model: null, effort: null, thread: null },
    ]);
    expect(allDone.label).toBe("Delegated results · Codex · completed");
    expect(allDone.failed).toBe(false);
    expect(wakeSummary([]).label).toBe("Delegated results");
  });
});
