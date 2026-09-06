import { describe, expect, it } from "vitest";

import type {
  AssistantMessageItem,
  ChatViewItem,
  PermissionRequestItem,
  ToolCallItem,
} from "@/lib/agent-chat/types";
import { assistantReferenceCwds, assistantReferencePaths } from "@/lib/agent-chat/reference-cwd";
import { deriveAlwaysRenderKeys } from "./always-render-keys";
import { buildTranscriptSlots } from "./transcript-slots";
import { getTranscriptHistory, getTranscriptPresentation } from "./transcript-derivations";

function assistant(seq = 3, text = "result"): AssistantMessageItem {
  return { kind: "assistant_message", id: `a${seq}`, seq, turn_id: "t", text, streaming: false };
}
function tool(overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call", id: "tool", seq: 2, tool_use_id: "use", tool_name: "Bash",
    input: { cwd: "/repo", command: "touch /repo/result.txt" }, status: "done",
    result_content: null, approval_request_id: null, ...overrides,
  };
}
function request(resolution: PermissionRequestItem["resolution"] = { state: "pending" }): PermissionRequestItem {
  return {
    kind: "permission_request", id: "request", seq: 4, request_id: "approval",
    request_kind: "command", turn_id: "t", tool_use_id: "use", payload: {}, resolution,
  };
}
function settledTurn(): ChatViewItem[] {
  return [
    { kind: "user_message", id: "user", seq: 1, text: "go", created_at: 1000 },
    tool(), assistant(),
    { kind: "turn_ended", id: "end", seq: 5, turn_id: "t", status: { kind: "success" }, completed_at: 2000 },
  ];
}

function expectOriginalPresentation(messages: ChatViewItem[], streaming: boolean, expanded: ReadonlySet<string>) {
  const history = getTranscriptHistory(messages);
  const actual = getTranscriptPresentation(history, streaming, expanded);
  const expected = buildTranscriptSlots(history.ordered, streaming, expanded);
  expect(actual.slots).toEqual(expected);
  expect(actual.alwaysRenderKeys).toEqual(deriveAlwaysRenderKeys(expected, history.requestsById));
  expect(history.referenceCwdByMessageId).toEqual(assistantReferenceCwds(history.ordered));
  expect(history.referencePathsByMessageId).toEqual(assistantReferencePaths(history.ordered));
  return actual;
}

describe("immutable transcript derivations", () => {
  it("returns identical history/maps for an identity hit without reading its rows", () => {
    let reads = 0;
    const messages = new Proxy(Object.freeze(settledTurn()), {
      get(target, property, receiver) {
        reads += 1;
        return Reflect.get(target, property, receiver);
      },
    });
    const first = getTranscriptHistory(messages);
    reads = 0;
    expect(getTranscriptHistory(messages)).toBe(first);
    expect(reads).toBe(0);
    expect(first.referenceCwdByMessageId.get("a3")).toBe("/repo");
    expect(first.referencePathsByMessageId.get("a3")).toEqual(["/repo/result.txt"]);
  });

  it("sorts a copy by seq and id and never keys by message IDs alone", () => {
    const a = assistant(3);
    const b = { ...a, id: "b3" };
    const input = Object.freeze([b, a]);
    const first = getTranscriptHistory(input);
    expect(first.ordered).toEqual([a, b]);
    expect(input).toEqual([b, a]);
    const second = getTranscriptHistory([{ ...a, text: "another provider's answer" }, b]);
    expect(second).not.toBe(first);
    expect(second.ordered[0]).toMatchObject({ text: "another provider's answer" });
  });

  it("keeps reference props and unaffected slots identical across a live delta", () => {
    const messages = [tool(), assistant()];
    const firstHistory = getTranscriptHistory(messages);
    const first = getTranscriptPresentation(firstHistory, true, new Set());
    const changed = [messages[0], { ...assistant(), text: "result grows", streaming: true }];
    const nextHistory = getTranscriptHistory(changed, firstHistory);
    const next = getTranscriptPresentation(nextHistory, true, new Set(), first.slots);
    expect(nextHistory.referenceCwdByMessageId).toBe(firstHistory.referenceCwdByMessageId);
    expect(nextHistory.referencePathsByMessageId).toBe(firstHistory.referencePathsByMessageId);
    expect(next.slots[0]).toBe(first.slots[0]);
    expect(next.slots[1]).not.toBe(first.slots[1]);
    expect(next.slots[1].body).toMatchObject({ item: { text: "result grows", streaming: true } });
    expect(getTranscriptPresentation(getTranscriptHistory(messages), true, new Set())).toBe(first);
  });

  it("separates streaming modes for the same snapshot including quiet live tools", () => {
    const messages = [tool({ tool_name: "Read", status: "running" })];
    const idle = expectOriginalPresentation(messages, false, new Set());
    const live = expectOriginalPresentation(messages, true, new Set());
    expect(live).not.toBe(idle);
    expect(live.slots[0].body).toMatchObject({ kind: "activity", working: true });
    expect(getTranscriptPresentation(getTranscriptHistory(messages), false, new Set())).toBe(idle);
    expect(getTranscriptPresentation(getTranscriptHistory(messages), true, new Set())).toBe(live);
  });

  it("leaves disclosure choices outside the shared collapsed cache", () => {
    const messages = settledTurn();
    const collapsed = expectOriginalPresentation(messages, false, new Set());
    const expanded = expectOriginalPresentation(messages, false, new Set(["t"]));
    expect(expanded.slots.length).toBeGreaterThan(collapsed.slots.length);
    expect(expanded.slots.some((slot) => slot.body.kind === "turn_fold" && slot.body.expanded)).toBe(true);
    const history = getTranscriptHistory(messages);
    expect(getTranscriptPresentation(history, false, new Set(["t"]))).not.toBe(expanded);
    expect(getTranscriptPresentation(history, false, new Set(), expanded.slots)).toBe(collapsed);
  });

  it("refreshes approval controls and gated reference context on immutable resolution", () => {
    const gated = tool({ approval_request_id: "approval" });
    const pending: ChatViewItem[] = [gated, assistant(), request()];
    const first = expectOriginalPresentation(pending, false, new Set());
    expect(first.alwaysRenderKeys).toContain("tool");
    expect(getTranscriptHistory(pending).referenceCwdByMessageId.size).toBe(0);
    const approved: ChatViewItem[] = [gated, pending[1], request({ state: "resolved", decision: { decision: "allow" } })];
    const next = expectOriginalPresentation(approved, false, new Set());
    expect(next.alwaysRenderKeys).toEqual([]);
    expect(getTranscriptHistory(approved).referenceCwdByMessageId.get("a3")).toBe("/repo");
    expect(getTranscriptHistory(pending).requestsById.get("approval")?.resolution.state).toBe("pending");
  });

  it("keeps chronological last-wins request and subagent-name lookup semantics", () => {
    const older = request();
    const newer = { ...request({ state: "responding", decision: { decision: "allow" } }), id: "new-request", seq: 6 };
    const messages: ChatViewItem[] = [newer, {
      kind: "subagent_run", id: "run", seq: 5, turn_id: "t",
      subagents: [
        { id: "named", name: "Worker", status: "running", items: [], toneIndex: 0 },
        { id: "typed", agentType: "explorer", status: "running", items: [], toneIndex: 1 },
        { id: "unnamed", status: "running", items: [], toneIndex: 2 },
      ],
    }, older];
    const history = getTranscriptHistory(messages);
    expect(history.requestsById.get("approval")).toBe(newer);
    expect([...history.subagentNames]).toEqual([
      ["named", "Worker"], ["typed", "explorer"], ["unnamed", "subagent"],
    ]);
    expect(getTranscriptHistory(messages)).toBe(history);
  });

  it("preserves queued controls in both streaming variants", () => {
    const messages: ChatViewItem[] = [...settledTurn(), { kind: "user_message", id: "queue", seq: 6, text: "next", queued: { queuedId: "q" } }];
    for (const streaming of [false, true]) {
      const presentation = expectOriginalPresentation(messages, streaming, new Set());
      expect(presentation.alwaysRenderKeys).toContain("queue");
    }
  });
});
