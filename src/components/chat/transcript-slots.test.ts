import { describe, it, expect } from "vitest";

import type {
  ChatViewItem,
  PermissionRequestItem,
  ReasoningItem,
  ToolCallItem,
} from "@/lib/agent-chat/types";

import { buildTranscriptSlots } from "./transcript-slots";

function userMsg(seq: number, text = "hi"): ChatViewItem {
  return { kind: "user_message", id: `um-${seq}`, seq, text };
}

function assistantMsg(seq: number, text = "ok"): ChatViewItem {
  return {
    kind: "assistant_message",
    id: `am-${seq}`,
    seq,
    turn_id: "t1",
    text,
    streaming: false,
  };
}

function reasoning(seq: number, overrides: Partial<ReasoningItem> = {}): ReasoningItem {
  return {
    kind: "reasoning",
    id: `re-${seq}`,
    seq,
    turn_id: "t1",
    text: "thinking about it",
    streaming: false,
    ...overrides,
  };
}

function tool(seq: number, overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call",
    id: `tc-${seq}`,
    seq,
    tool_use_id: `tu-${seq}`,
    tool_name: "Read",
    input: { file_path: `/f${seq}` },
    status: "done",
    result_content: null,
    approval_request_id: null,
    ...overrides,
  };
}

describe("buildTranscriptSlots — activity grouping", () => {
  it("folds a run of ≥2 completed tool calls into one activity slot", () => {
    const slots = buildTranscriptSlots([tool(0), tool(1), tool(2)]);
    expect(slots).toHaveLength(1);
    expect(slots[0].body.kind).toBe("activity");
    if (slots[0].body.kind === "activity") {
      expect(slots[0].body.items).toHaveLength(3);
      expect(slots[0].body.working).toBe(false);
    }
    // Key is derived from the first step's id (stable as the run grows).
    expect(slots[0].messageId).toBe("run:tc-0");
  });

  it("keeps a lone completed tool call as its own item slot (below GROUP_MIN)", () => {
    const slots = buildTranscriptSlots([tool(0)]);
    expect(slots).toHaveLength(1);
    expect(slots[0].body.kind).toBe("item");
  });

  it("folds contiguous reasoning + tool calls into ONE activity block", () => {
    const slots = buildTranscriptSlots([reasoning(0), tool(1), reasoning(2), tool(3)]);
    expect(slots).toHaveLength(1);
    expect(slots[0].body.kind).toBe("activity");
    if (slots[0].body.kind === "activity") {
      expect(slots[0].body.items.map((s) => s.kind)).toEqual([
        "reasoning",
        "tool_call",
        "reasoning",
        "tool_call",
      ]);
    }
    // Keyed off the first step (a reasoning item here).
    expect(slots[0].messageId).toBe("run:re-0");
  });

  it("keeps a lone reasoning run (no tools) as ReasoningBlock items, not an activity block", () => {
    const slots = buildTranscriptSlots([reasoning(0), reasoning(1)]);
    expect(slots.map((s) => s.body.kind)).toEqual(["item", "item"]);
  });

  it("folds running, errored and edit tool calls in as steps", () => {
    const running = tool(1, { status: "running" });
    const errored = tool(2, { status: "error" });
    const edit = tool(3, {
      tool_name: "Edit",
      input: { file_path: "/a", old_string: "x", new_string: "y" },
    });
    const slots = buildTranscriptSlots([tool(0), running, errored, edit]);
    expect(slots).toHaveLength(1);
    expect(slots[0].body.kind).toBe("activity");
    if (slots[0].body.kind === "activity") {
      expect(slots[0].body.items).toHaveLength(4);
    }
  });

  it("shows a single running tool as a WORKING activity block on the streaming tail (GROUP_MIN drops to 1)", () => {
    const slots = buildTranscriptSlots([tool(0, { status: "running" })], true);
    expect(slots).toHaveLength(1);
    expect(slots[0].body.kind).toBe("activity");
    if (slots[0].body.kind === "activity") {
      expect(slots[0].body.working).toBe(true);
      expect(slots[0].body.items).toHaveLength(1);
    }
  });

  it("keeps a lone tool as a single card when settled (not streaming)", () => {
    const slots = buildTranscriptSlots([tool(0, { status: "running" })], false);
    expect(slots[0].body.kind).toBe("item");
  });

  it("marks working only on the tail run of an active turn; earlier runs settle", () => {
    const slots = buildTranscriptSlots(
      [tool(0), tool(1), assistantMsg(2), tool(3, { status: "running" })],
      true,
    );
    expect(slots.map((s) => s.body.kind)).toEqual(["activity", "item", "activity"]);
    if (slots[0].body.kind === "activity") expect(slots[0].body.working).toBe(false);
    if (slots[2].body.kind === "activity") expect(slots[2].body.working).toBe(true);
  });

  it("never absorbs an approval-gated tool call — it stays standalone", () => {
    const gated = tool(2, { status: "running", approval_request_id: "req-x" });
    const slots = buildTranscriptSlots([tool(0), tool(1), gated]);
    // Two reads fold into an activity; the gated tool is its own item.
    expect(slots.map((s) => s.body.kind)).toEqual(["activity", "item"]);
    expect(slots[1].messageId).toBe("tc-2");
  });

  it("never absorbs a TodoWrite / task-summary tool", () => {
    const todo = tool(2, {
      tool_name: "TodoWrite",
      input: { todos: [{ content: "a", status: "pending" }] },
    });
    const slots = buildTranscriptSlots([tool(0), tool(1), todo]);
    expect(slots.map((s) => s.body.kind)).toEqual(["activity", "item"]);
    expect(slots[1].messageId).toBe("tc-2");
  });

  it("breaks a run at a non-step item, yielding two activity blocks", () => {
    const slots = buildTranscriptSlots([
      tool(0),
      tool(1),
      assistantMsg(2),
      tool(3),
      tool(4),
    ]);
    expect(slots.map((s) => s.body.kind)).toEqual(["activity", "item", "activity"]);
  });
});

describe("buildTranscriptSlots — non-rendering rows", () => {
  it("drops a permission request already merged into a tool card footer", () => {
    const gated = tool(0, { status: "running", approval_request_id: "req-1" });
    const merged: PermissionRequestItem = {
      kind: "permission_request",
      id: "req-1",
      seq: 1,
      request_id: "req-1",
      turn_id: "t1",
      request_kind: "command",
      payload: {},
      tool_use_id: "tu-0",
      resolution: { state: "pending" },
    };
    const slots = buildTranscriptSlots([gated, merged]);
    // Only the gated tool card row survives (standalone item).
    expect(slots).toHaveLength(1);
    expect(slots[0].messageId).toBe("tc-0");
  });

  it("drops a non-error turn_ended row but keeps an error one", () => {
    const ok: ChatViewItem = {
      kind: "turn_ended",
      id: "te-ok",
      seq: 0,
      turn_id: "t1",
      status: { kind: "success" },
    };
    const bad: ChatViewItem = {
      kind: "turn_ended",
      id: "te-bad",
      seq: 1,
      turn_id: "t2",
      status: { kind: "error", subtype: "boom", message: "kaboom" },
    };
    const slots = buildTranscriptSlots([ok, bad]);
    expect(slots).toHaveLength(1);
    expect(slots[0].messageId).toBe("te-bad");
  });
});

describe("buildTranscriptSlots — subagent card", () => {
  function subagentRun(seq: number): ChatViewItem {
    return {
      kind: "subagent_run",
      id: `run-${seq}`,
      seq,
      turn_id: "t1",
      subagents: [
        {
          id: "a",
          status: "running",
          items: [],
          toneIndex: 0,
        },
      ],
    };
  }

  it("renders a subagent_run as its own standalone item slot", () => {
    const slots = buildTranscriptSlots([subagentRun(0)]);
    expect(slots).toHaveLength(1);
    expect(slots[0].body.kind).toBe("item");
    expect(slots[0].messageId).toBe("run-0");
    // Assistant-side, but never a scroll anchor (not a user turn).
    expect(slots[0].side).toBe("assistant");
    expect(slots[0].scrollAnchor).toBe(false);
  });

  it("breaks a run — the card never folds into an activity block", () => {
    const slots = buildTranscriptSlots([
      tool(0),
      tool(1),
      subagentRun(2),
      tool(3),
      tool(4),
    ]);
    // The subagent_run is a standalone `item` slot that splits the two
    // contiguous tool runs into separate Activity blocks (#124 renamed the
    // folded slot kind `toolGroup` → `activity`).
    expect(slots.map((s) => s.body.kind)).toEqual([
      "activity",
      "item",
      "activity",
    ]);
  });
});

describe("buildTranscriptSlots — turn boundaries", () => {
  it("marks user rows as scroll anchors on the user side", () => {
    const slots = buildTranscriptSlots([userMsg(0)]);
    expect(slots[0].side).toBe("user");
    expect(slots[0].scrollAnchor).toBe(true);
    expect(slots[0].turnStart).toBe(true);
    expect(slots[0].showAvatar).toBe(false);
  });

  it("shows the avatar once — on the first assistant row of a contiguous run", () => {
    const slots = buildTranscriptSlots([
      userMsg(0),
      assistantMsg(1),
      assistantMsg(2),
      tool(3),
    ]);
    const [u, a1, a2, t3] = slots;
    expect(u.showAvatar).toBe(false);
    expect(a1.showAvatar).toBe(true);
    expect(a1.turnStart).toBe(true);
    expect(a2.showAvatar).toBe(false);
    expect(a2.turnStart).toBe(false);
    expect(t3.showAvatar).toBe(false);
    expect(t3.scrollAnchor).toBe(false);
  });

  it("starts a fresh assistant turn (new avatar) after a user interjection", () => {
    const slots = buildTranscriptSlots([
      assistantMsg(0),
      userMsg(1),
      assistantMsg(2),
    ]);
    expect(slots[0].showAvatar).toBe(true);
    expect(slots[2].showAvatar).toBe(true);
    expect(slots[2].turnStart).toBe(true);
  });
});
