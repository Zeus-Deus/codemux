import { describe, expect, it } from "vitest";

import { delegationRow } from "@/lib/agent-chat/delegation";
import type {
  ChatViewItem,
  SubagentRunItem,
  SubagentView,
  ToolCallItem,
} from "@/lib/agent-chat/types";

import {
  buildTranscriptSlots,
  reuseTranscriptSlots,
  type DelegationEntry,
  type TranscriptSlot,
} from "./transcript-slots";

function user(seq: number, text = "have Codex do it"): ChatViewItem {
  return { kind: "user_message", id: `um-${seq}`, seq, text, created_at: 0 };
}

function assistant(seq: number, text = "ok"): ChatViewItem {
  return { kind: "assistant_message", id: `am-${seq}`, seq, turn_id: "t1", text, streaming: false };
}

function delegateCall(seq: number, thread: string | null, provider = "codex"): ToolCallItem {
  return {
    kind: "tool_call",
    id: `tc-${seq}`,
    seq,
    turn_id: "t1",
    tool_use_id: `tu-${seq}`,
    tool_name: "mcp__codemux__delegate_task",
    input: { provider, title: `Task ${seq}`, task: "A complete, self-contained brief." },
    status: thread ? "done" : "running",
    result_content: thread
      ? [{ type: "text", text: JSON.stringify({ started: { provider, thread } }) }]
      : null,
    approval_request_id: null,
  };
}

function readTool(seq: number): ToolCallItem {
  return {
    kind: "tool_call",
    id: `tc-${seq}`,
    seq,
    tool_use_id: `tu-${seq}`,
    tool_name: "Bash",
    input: { command: "ls" },
    status: "done",
    result_content: null,
    approval_request_id: null,
  };
}

function row(
  thread: string,
  status: SubagentView["status"] = "running",
  agentType = "codex",
): SubagentView {
  return {
    id: `delegate:${thread}`,
    agentType,
    status,
    items: [],
    toneIndex: 0,
  };
}

function card(seq: number, subagents: SubagentView[]): SubagentRunItem {
  return { kind: "subagent_run", id: `sr-${seq}`, seq, turn_id: "t1", subagents };
}

function turnEnd(seq: number): ChatViewItem {
  return {
    kind: "turn_ended",
    id: `te-${seq}`,
    seq,
    turn_id: "t1",
    status: { kind: "success" },
    completed_at: 2_000,
  };
}

function delegationSlots(slots: TranscriptSlot[]): DelegationEntry[][] {
  return slots.flatMap((slot) =>
    slot.body.kind === "delegation" ? [slot.body.entries] : [],
  );
}

describe("delegation slots", () => {
  it("renders a delegation once: the call, joined to its live row", () => {
    const view = row("c1");
    const slots = buildTranscriptSlots(
      [user(0), assistant(1), delegateCall(2, "c1"), card(3, [view])],
      true,
    );
    const groups = delegationSlots(slots);
    expect(groups).toHaveLength(1);
    expect(groups[0]).toHaveLength(1);
    expect(groups[0][0].call?.id).toBe("tc-2");
    expect(groups[0][0].view).toBe(view);
    // Neither the call nor the card leaks into the work log.
    expect(slots.some((slot) => slot.body.kind === "activity")).toBe(false);
  });

  it("shows a call still starting on its own, before any row exists", () => {
    const groups = delegationSlots(
      buildTranscriptSlots([user(0), delegateCall(1, null)], true),
    );
    expect(groups).toEqual([[{ key: "tc-1", call: expect.anything(), view: null }]]);
  });

  it("groups contiguous delegations, however the calls and cards interleave", () => {
    const slots = buildTranscriptSlots(
      [
        user(0),
        delegateCall(1, "c1"),
        card(2, [row("c1")]),
        delegateCall(3, "c2"),
        card(4, [row("c2")]),
      ],
      true,
    );
    const groups = delegationSlots(slots);
    expect(groups).toHaveLength(1);
    expect(groups[0].map((entry) => entry.key)).toEqual(["tc-1", "tc-3"]);
  });

  it("splits groups at prose and at mechanical work", () => {
    const slots = buildTranscriptSlots(
      [
        user(0),
        delegateCall(1, "c1"),
        card(2, [row("c1")]),
        assistant(3, "and another"),
        delegateCall(4, "c2"),
        card(5, [row("c2")]),
        readTool(6),
        delegateCall(7, "c3"),
      ],
      true,
    );
    expect(delegationSlots(slots).map((group) => group.map((entry) => entry.key))).toEqual([
      ["tc-1"],
      ["tc-4"],
      ["tc-7"],
    ]);
    expect(slots.some((slot) => slot.body.kind === "activity")).toBe(true);
  });

  it("keeps the card outside a settled turn's fold", () => {
    const slots = buildTranscriptSlots([
      user(0),
      assistant(1, "handing off"),
      readTool(2),
      delegateCall(3, "c1"),
      card(4, [row("c1")]),
      assistant(5, "Codex is on it."),
      turnEnd(6),
    ]);
    const kinds = slots.map((slot) => slot.body.kind);
    expect(kinds).toEqual(["item", "turn_fold", "delegation", "item"]);
    // A finished delegation stays put too.
    const done = buildTranscriptSlots([
      user(0),
      delegateCall(1, "c1"),
      card(2, [row("c1", "completed")]),
      assistant(3, "Codex is on it."),
      turnEnd(4),
    ]);
    expect(delegationSlots(done)).toHaveLength(1);
  });

  it("does not split the work log at a card whose rows a call already shows", () => {
    // One message: delegate_task and a Bash call, then the row's snapshot.
    const slots = buildTranscriptSlots(
      [user(0), delegateCall(1, "c1"), readTool(2), card(3, [row("c1")]), readTool(4)],
      true,
    );
    expect(slots.map((slot) => slot.body.kind)).toEqual(["item", "delegation", "activity"]);
    const activity = slots[2].body;
    expect(activity.kind === "activity" && activity.items.map((item) => item.id)).toEqual([
      "tc-2",
      "tc-4",
    ]);
  });

  it("keeps a row that lands before its call (Codex) on the call's entry", () => {
    const pending = row("c1", "pending");
    const slots = buildTranscriptSlots([user(0), card(1, [pending]), delegateCall(2, null)], true);
    expect(delegationSlots(slots)).toEqual([[{ key: "tc-2", call: expect.anything(), view: pending }]]);
  });

  it("shows parallel calls still in flight once each, joined to their rows", () => {
    const codex = row("c1", "pending");
    const claude = row("c2", "pending", "claude");
    const groups = delegationSlots(
      buildTranscriptSlots(
        [user(0), delegateCall(1, null), delegateCall(2, null, "claude"), card(3, [codex, claude])],
        true,
      ),
    );
    expect(groups).toHaveLength(1);
    expect(groups[0].map((entry) => [entry.key, entry.view])).toEqual([
      ["tc-1", codex],
      ["tc-2", claude],
    ]);
  });

  it("shows a refused call as failed beside the sibling that started", () => {
    const refused: ToolCallItem = {
      ...delegateCall(1, null),
      status: "error",
      result_content: [{ type: "text", text: "Delegation is off." }],
    };
    const started = row("c1", "pending");
    const [group] = delegationSlots(
      buildTranscriptSlots([user(0), refused, delegateCall(2, null), card(3, [started])], true),
    );
    expect(group.map((entry) => [entry.key, entry.view])).toEqual([
      ["tc-1", null],
      ["tc-2", started],
    ]);
    expect(delegationRow(group[0].call, group[0].view)).toMatchObject({
      phase: "failed",
      line: "Delegation is off.",
    });
  });

  it("keeps a gated call's approval prompt beside the card", () => {
    // The card has no approval footer, so the standalone prompt must stay.
    const gated: ToolCallItem = { ...delegateCall(1, null), approval_request_id: "req-1" };
    const request: ChatViewItem = {
      kind: "permission_request",
      id: "pr-2",
      seq: 2,
      turn_id: "t1",
      request_id: "req-1",
      request_kind: "tool",
      payload: {},
      tool_use_id: "tu-1",
      resolution: { state: "pending" },
    };
    const slots = buildTranscriptSlots([user(0), gated, request], true);
    expect(slots.map((slot) => slot.body.kind)).toEqual(["item", "delegation", "item"]);
    const last = slots[2].body;
    expect(last.kind === "item" && last.item.id).toBe("pr-2");
  });

  it("renders a row whose call is gone at the row's own position", () => {
    const orphan = row("lost");
    const slots = buildTranscriptSlots([user(0), assistant(1), card(2, [orphan])], true);
    expect(delegationSlots(slots)).toEqual([[{ key: "delegate:lost", call: null, view: orphan }]]);
  });

  it("leaves native subagents in the work log", () => {
    const native: SubagentView = { id: "explore", status: "running", items: [], toneIndex: 0 };
    const slots = buildTranscriptSlots([user(0), card(1, [native])], true);
    expect(slots.map((slot) => slot.body.kind)).toEqual(["item", "activity"]);
  });

  it("keeps an untouched card's slot identity across rebuilds", () => {
    const messages = [user(0), delegateCall(1, "c1"), card(2, [row("c1")])];
    const first = buildTranscriptSlots(messages, true);
    const again = reuseTranscriptSlots(first, buildTranscriptSlots(messages, true));
    expect(again).toBe(first);
    const updated = [user(0), delegateCall(1, "c1"), card(2, [row("c1", "completed")])];
    const next = reuseTranscriptSlots(first, buildTranscriptSlots(updated, true));
    expect(next).not.toBe(first);
    expect(next[1].key).toBe(first[1].key);
  });
});
