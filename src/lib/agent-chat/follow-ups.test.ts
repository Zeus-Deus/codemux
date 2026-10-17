import { describe, expect, it } from "vitest";

import type { FollowUpTaskRow } from "@/tauri/commands";
import type { ChatModelInfo } from "@/tauri/types";

import {
  defaultFollowUpRun,
  doItHereTurn,
  isSuggestToolCall,
  linkSuggestedTasks,
  mergeableCount,
  mergeSummary,
  resolveFollowUpEffort,
  startedTaskState,
  suggestedTasksOf,
  viewsForCall,
} from "./follow-ups";
import type { ChatViewItem, SubagentRunItem, SubagentView, ToolCallItem } from "./types";

const TASKS = [
  { title: "Add retry tests", task: "Add table tests for the refresh retry path.", provider: "codex" },
  { title: "", task: "Document the auth flow\nin docs/auth.md with a diagram." },
];

function call(overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call",
    id: "tc-1",
    seq: 1,
    turn_id: "t1",
    tool_use_id: "call-1",
    tool_name: "mcp__codemux__mcp__codemux__suggest_tasks",
    input: { tasks: TASKS },
    status: "done",
    result_content: null,
    approval_request_id: null,
    started_at: 1,
    ...overrides,
  } as ToolCallItem;
}

function view(overrides: Partial<SubagentView> = {}): SubagentView {
  return {
    id: "delegate:child-1",
    parentItemId: "call-1#0",
    agentType: "codex",
    name: "Codex",
    description: "Add retry tests",
    status: "running",
    activity: "Working in its workspace",
    ...overrides,
  } as SubagentView;
}

function run(views: SubagentView[]): SubagentRunItem {
  return { kind: "subagent_run", id: "run-1", seq: 2, subagents: views } as SubagentRunItem;
}

function row(overrides: Partial<FollowUpTaskRow> = {}): FollowUpTaskRow {
  return {
    child_thread_id: "child-1",
    parent_thread_id: "parent",
    tool_call_id: "call-1",
    index: 0,
    workspace_id: "ws-child",
    parent_workspace_id: "ws-parent",
    repo_path: "/repo",
    worktree_path: "/wt",
    branch: "add-retry-tests",
    base_sha: "a".repeat(40),
    provider: "codex",
    title: "Add retry tests",
    outcome: "running",
    additions: null,
    deletions: null,
    commits: null,
    uncommitted: null,
    merged: false,
    created_at_ms: 1,
    ...overrides,
  };
}

describe("suggested tasks", () => {
  it("reads Claude and Codex calls and ignores other tools", () => {
    const tasks = suggestedTasksOf(call());
    expect(tasks).toEqual([
      { title: "Add retry tests", task: TASKS[0].task, provider: "codex" },
      { title: "Document the auth flow", task: TASKS[1].task, provider: null },
    ]);
    const codex = call({
      tool_name: "dynamicToolCall",
      input: { tool: "codemux_mcp__codemux__suggest_tasks", arguments: JSON.stringify({ tasks: TASKS }) },
    });
    expect(suggestedTasksOf(codex)).toHaveLength(2);
    expect(suggestedTasksOf(call({ tool_name: "mcp__other__suggest_tasks" }))).toBeNull();
    expect(suggestedTasksOf(call({ tool_name: "mcp__my-codemux__suggest_tasks" }))).toBeNull();
  });

  it("is a card only when it parsed and was not refused", () => {
    expect(isSuggestToolCall(call())).toBe(true);
    expect(isSuggestToolCall(call({ status: "error" }))).toBe(false);
    expect(isSuggestToolCall(call({ input: { tasks: [] } }))).toBe(false);
  });

  it("links started rows to their suggestion and claims them", () => {
    const started = view();
    const other = view({ id: "delegate:x", parentItemId: "other-call#0" });
    const tab = view({ id: "delegate:tab", parentItemId: undefined });
    const messages: ChatViewItem[] = [call(), run([started, other, tab])];
    const { viewByKey, claimed } = linkSuggestedTasks(messages);
    expect(viewByKey.get("call-1#0")).toBe(started);
    expect([...claimed]).toEqual(["delegate:child-1"]);
    expect(viewsForCall(messages[0] as ToolCallItem, viewByKey)).toEqual([started, null]);
  });

  it("merges the durable row with the live row", () => {
    expect(startedTaskState(null, null)).toBeNull();
    const starting = startedTaskState(row(), null)!;
    expect(starting.phase).toBe("starting");
    expect(starting.line).toBe("Starting its workspace…");
    expect(starting.workspaceId).toBe("ws-child");
    const live = startedTaskState(row(), view({ activity: "Waiting for your answer in its workspace" }))!;
    expect(live.phase).toBe("waiting");
    expect(live.live).toBe(true);
    const done = startedTaskState(
      row({ outcome: "completed", additions: 12, deletions: 3 }),
      view({ status: "completed", resultText: "Added 4 tests." }),
    )!;
    expect(done.phase).toBe("done");
    expect(done.diff).toBe("+12 −3");
    expect(done.live).toBe(false);
  });

  it("counts mergeable rows and phrases merge results", () => {
    expect(
      mergeableCount([
        row({ outcome: "completed" }),
        row({ outcome: "completed", merged: true }),
        row({ outcome: "running" }),
      ]),
    ).toBe(1);
    expect(
      mergeSummary({
        target_branch: "main",
        merged: [{ title: "A" }, { title: "B" }],
        skipped: [],
        conflict: null,
      }),
    ).toEqual({ text: "Merged 2 branches into main.", failed: false });
    const conflict = mergeSummary({
      target_branch: "main",
      merged: [{ title: "A" }],
      skipped: [],
      conflict: { title: "B", branch: "b", files: ["src/a.ts"], message: null },
    });
    expect(conflict.failed).toBe(true);
    expect(conflict.text).toContain("Merged 1 branch into main.");
    expect(conflict.text).toContain("conflicts in src/a.ts");
    expect(conflict.text).toContain("aborted");
  });

  it("sends the full brief but shows a short line for Do it here", () => {
    const turn = doItHereTurn({ title: "Add retry tests", task: "Brief.", provider: null });
    expect(turn.text).toContain("Brief.");
    expect(turn.displayText).toBe("Do the suggested task here: Add retry tests");
  });

  it("defaults a task to the parent chat's run, or the named provider's defaults", () => {
    const base = {
      allowed: ["claude", "codex"] as const,
      chatProvider: "claude" as const,
      chatModel: "claude-opus-5-5",
      chatEffort: "high",
      defaultModel: (provider: string) => (provider === "codex" ? "gpt-6" : "claude-default"),
    };
    const task = (provider: "claude" | "codex" | "cursor" | null) => ({ title: "T", task: "B", provider });
    expect(defaultFollowUpRun({ ...base, task: task(null) })).toEqual({
      provider: "claude",
      model: "claude-opus-5-5",
      effort: "high",
    });
    expect(defaultFollowUpRun({ ...base, task: task("claude") }).model).toBe("claude-opus-5-5");
    expect(defaultFollowUpRun({ ...base, task: task("codex") })).toEqual({
      provider: "codex",
      model: "gpt-6",
      effort: null,
    });
    // A provider the backend can't run is ignored.
    expect(defaultFollowUpRun({ ...base, task: task("cursor") }).provider).toBe("claude");
    // A chat on a provider follow-ups can't use falls back to the first allowed one.
    expect(
      defaultFollowUpRun({ ...base, chatProvider: "cursor" as never, task: task(null) }),
    ).toEqual({ provider: "claude", model: "claude-default", effort: null });
  });

  it("pins effort to one the model accepts", () => {
    const info = { effort_levels: ["low", "high"], default_effort: "low" } as ChatModelInfo;
    expect(resolveFollowUpEffort(info, "high")).toBe("high");
    expect(resolveFollowUpEffort(info, "max")).toBe("low");
    expect(resolveFollowUpEffort(info, null)).toBe("low");
    expect(resolveFollowUpEffort(null, "max")).toBe("max");
  });

  it("carries the model a started task runs on", () => {
    const state = startedTaskState(row(), view({ model: "gpt-6", effort: "high" }))!;
    expect(state).toMatchObject({ provider: "codex", model: "gpt-6", effort: "high" });
    expect(startedTaskState(row(), null)).toMatchObject({ provider: "codex", model: null });
  });
});
