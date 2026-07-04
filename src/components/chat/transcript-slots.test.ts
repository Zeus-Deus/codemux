import { describe, it, expect } from "vitest";

import type {
  ChatViewItem,
  PermissionRequestItem,
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

describe("buildTranscriptSlots — grouping", () => {
  it("folds a run of ≥2 completed tool calls into one toolGroup slot", () => {
    const slots = buildTranscriptSlots([tool(0), tool(1), tool(2)]);
    expect(slots).toHaveLength(1);
    expect(slots[0].body.kind).toBe("toolGroup");
    if (slots[0].body.kind === "toolGroup") {
      expect(slots[0].body.items).toHaveLength(3);
    }
    // Group messageId is derived from the first member's id.
    expect(slots[0].messageId).toBe("run:tc-0");
  });

  it("keeps a lone completed tool call as its own item slot", () => {
    const slots = buildTranscriptSlots([tool(0)]);
    expect(slots).toHaveLength(1);
    expect(slots[0].body.kind).toBe("item");
  });

  it("breaks a run at a non-tool item, yielding two groups", () => {
    const slots = buildTranscriptSlots([
      tool(0),
      tool(1),
      assistantMsg(2),
      tool(3),
      tool(4),
    ]);
    const kinds = slots.map((s) => s.body.kind);
    expect(kinds).toEqual(["toolGroup", "item", "toolGroup"]);
  });

  it("excludes running, error, pending-approval, TodoWrite and diff calls from groups", () => {
    const running = tool(1, { status: "running" });
    const errored = tool(2, { status: "error" });
    const gated = tool(3, { approval_request_id: "req-x" });
    const todo = tool(4, {
      tool_name: "TodoWrite",
      input: { todos: [{ content: "a", status: "pending" }] },
    });
    const edit = tool(5, {
      tool_name: "Edit",
      input: { file_path: "/a", old_string: "x", new_string: "y" },
    });
    const slots = buildTranscriptSlots([
      tool(0),
      running,
      errored,
      gated,
      todo,
      edit,
    ]);
    // The lone completed tool(0) flushes as a single item (run of 1), then
    // each non-groupable call is its own item — no group forms.
    expect(slots.every((s) => s.body.kind === "item")).toBe(true);
    expect(slots).toHaveLength(6);
  });

  it("keeps a diff tool between two read runs from swallowing the diff card", () => {
    const edit = tool(2, {
      tool_name: "Edit",
      input: { file_path: "/a", old_string: "x", new_string: "y" },
    });
    const slots = buildTranscriptSlots([
      tool(0),
      tool(1),
      edit,
      tool(3),
      tool(4),
    ]);
    // group(reads) · edit item · group(reads)
    expect(slots.map((s) => s.body.kind)).toEqual([
      "toolGroup",
      "item",
      "toolGroup",
    ]);
  });
});

describe("buildTranscriptSlots — non-rendering rows", () => {
  it("drops a permission request already merged into a tool card footer", () => {
    const gated = tool(0, {
      status: "running",
      approval_request_id: "req-1",
    });
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
    // Only the tool card row survives — the merged request is not its own row.
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
    // user, then the assistant run (message, message, single tool).
    const [u, a1, a2, t3] = slots;
    expect(u.showAvatar).toBe(false);
    expect(a1.showAvatar).toBe(true); // first assistant row → avatar + turnStart
    expect(a1.turnStart).toBe(true);
    expect(a2.showAvatar).toBe(false); // continuation → gutter, no avatar
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
