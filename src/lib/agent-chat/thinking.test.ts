import { describe, expect, it } from "vitest";

import { shouldShowThinkingIndicator } from "./thinking";
import type {
  AssistantMessageItem,
  ChatViewItem,
  PermissionRequestItem,
  ReasoningItem,
  ToolCallItem,
  UserMessageItem,
} from "./types";

function userMsg(seq: number): UserMessageItem {
  return { kind: "user_message", id: `u${seq}`, seq, text: "hi" };
}
function assistantMsg(
  seq: number,
  streaming: boolean,
  text = "",
): AssistantMessageItem {
  return {
    kind: "assistant_message",
    id: `a${seq}`,
    seq,
    turn_id: "t1",
    text,
    streaming,
  };
}
function toolCall(
  seq: number,
  status: ToolCallItem["status"],
): ToolCallItem {
  return {
    kind: "tool_call",
    id: `tc${seq}`,
    seq,
    tool_use_id: `tu-${seq}`,
    tool_name: "Read",
    input: {},
    status,
    result_content: null,
    approval_request_id: null,
  };
}
function req(
  seq: number,
  state: PermissionRequestItem["resolution"]["state"],
): PermissionRequestItem {
  const resolution =
    state === "pending"
      ? ({ state: "pending" } as const)
      : state === "responding"
        ? ({
            state: "responding",
            decision: { decision: "allow" },
          } as const)
        : ({
            state: "resolved",
            decision: { decision: "allow" },
          } as const);
  return {
    kind: "permission_request",
    id: `r${seq}`,
    seq,
    request_id: `req-${seq}`,
    turn_id: "t1",
    request_kind: "plan",
    payload: {},
    tool_use_id: null,
    resolution,
  };
}

function reasoning(seq: number, streaming: boolean): ReasoningItem {
  return {
    kind: "reasoning",
    id: `re${seq}`,
    seq,
    turn_id: "t1",
    text: "considering",
    streaming,
  };
}

describe("shouldShowThinkingIndicator", () => {
  it("is false when not streaming", () => {
    expect(shouldShowThinkingIndicator([userMsg(0)], false)).toBe(false);
  });

  it("is true when streaming and transcript is empty", () => {
    expect(shouldShowThinkingIndicator([], true)).toBe(true);
  });

  it("shows after a user message (waiting for agent)", () => {
    expect(shouldShowThinkingIndicator([userMsg(0)], true)).toBe(true);
  });

  it("hides while an assistant message is streaming", () => {
    const msgs: ChatViewItem[] = [userMsg(0), assistantMsg(1, true, "hi")];
    expect(shouldShowThinkingIndicator(msgs, true)).toBe(false);
  });

  it("shows when the trailing assistant message has sealed", () => {
    const msgs: ChatViewItem[] = [userMsg(0), assistantMsg(1, false, "done")];
    expect(shouldShowThinkingIndicator(msgs, true)).toBe(true);
  });

  it("hides while a tool call is running", () => {
    const msgs: ChatViewItem[] = [userMsg(0), toolCall(1, "running")];
    expect(shouldShowThinkingIndicator(msgs, true)).toBe(false);
  });

  it("shows after a tool call has finished (between-tool gap)", () => {
    const msgs: ChatViewItem[] = [userMsg(0), toolCall(1, "done")];
    expect(shouldShowThinkingIndicator(msgs, true)).toBe(true);
  });

  it("hides while a reasoning block is streaming (its own header shows Thinking…)", () => {
    const msgs: ChatViewItem[] = [userMsg(0), reasoning(1, true)];
    expect(shouldShowThinkingIndicator(msgs, true)).toBe(false);
  });

  it("shows after a reasoning block has sealed (between-activity gap)", () => {
    const msgs: ChatViewItem[] = [userMsg(0), reasoning(1, false)];
    expect(shouldShowThinkingIndicator(msgs, true)).toBe(true);
  });

  it("hides while a permission request is pending (user must act)", () => {
    expect(shouldShowThinkingIndicator([req(0, "pending")], true)).toBe(false);
  });

  it("shows after a permission request is responding (AskUserQuestion submitted)", () => {
    expect(shouldShowThinkingIndicator([req(0, "responding")], true)).toBe(
      true,
    );
  });

  it("shows after a permission request is resolved (plan accepted)", () => {
    expect(shouldShowThinkingIndicator([req(0, "resolved")], true)).toBe(true);
  });
});
