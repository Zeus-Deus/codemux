import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ProviderRuntimeEvent } from "@/tauri/events";

import {
  __resetReducerIdCounterForTests,
  applyEvent,
  appendUserMessage,
  createEmptyThreadState,
  markRequestResponding,
} from "./reducer";
import type {
  AssistantMessageItem,
  ChatThreadState,
  PermissionRequestItem,
  ToolCallItem,
} from "./types";

function runEvents(
  events: ProviderRuntimeEvent[],
  initial: ChatThreadState = createEmptyThreadState(),
): ChatThreadState {
  return events.reduce<ChatThreadState>(applyEvent, initial);
}

describe("agent-chat reducer", () => {
  beforeEach(() => {
    __resetReducerIdCounterForTests();
  });

  it("accumulates text deltas into a single assistant message", () => {
    const state = runEvents([
      {
        type: "session_state_changed",
        thread_id: "t1",
        status: { status: "running", active_turn: "turn-1" },
      },
      {
        type: "content_delta",
        thread_id: "t1",
        turn_id: "turn-1",
        delta: { kind: "text", text: "Hello" },
      },
      {
        type: "content_delta",
        thread_id: "t1",
        turn_id: "turn-1",
        delta: { kind: "text", text: ", world" },
      },
    ]);

    const assistants = state.messages.filter(
      (m): m is AssistantMessageItem => m.kind === "assistant_message",
    );
    expect(assistants).toHaveLength(1);
    expect(assistants[0].text).toBe("Hello, world");
    expect(assistants[0].streaming).toBe(true);
    expect(state.streaming).toBe(true);
  });

  it("item_completed(assistant_text) after deltas seals with the full final text (no duplication, no truncation)", () => {
    // The Rust producer emits content_delta chunks during streaming,
    // then a single item_completed(assistant_text) carrying the FULL
    // turn text (not just the last chunk). The reducer must REPLACE
    // the accumulated text with item.text — concatenating would
    // duplicate, dropping item.text would truncate.
    const state = runEvents([
      {
        type: "session_state_changed",
        thread_id: "t1",
        status: { status: "running", active_turn: "turn-1" },
      },
      {
        type: "content_delta",
        thread_id: "t1",
        turn_id: "turn-1",
        delta: { kind: "text", text: "Hello" },
      },
      {
        type: "content_delta",
        thread_id: "t1",
        turn_id: "turn-1",
        delta: { kind: "text", text: ", world" },
      },
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: { kind: "assistant_text", text: "Hello, world" },
      },
    ]);

    const assistants = state.messages.filter(
      (m): m is AssistantMessageItem => m.kind === "assistant_message",
    );
    expect(assistants).toHaveLength(1);
    expect(assistants[0].text).toBe("Hello, world");
    expect(assistants[0].streaming).toBe(false);
  });

  it("attaches a tool_result that arrives before its tool_use", () => {
    const state = runEvents([
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: {
          kind: "tool_result",
          tool_use_id: "tu-1",
          content: "42",
          is_error: false,
        },
      },
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: {
          kind: "tool_use",
          tool_use_id: "tu-1",
          tool_name: "Bash",
          input: { command: "echo 42" },
        },
      },
    ]);

    const tools = state.messages.filter(
      (m): m is ToolCallItem => m.kind === "tool_call",
    );
    expect(tools).toHaveLength(1);
    expect(tools[0].tool_name).toBe("Bash");
    expect(tools[0].status).toBe("done");
    expect(tools[0].result_content).toBe("42");
  });

  it("appends a new tool_call item when tool_use arrives first", () => {
    const state = runEvents([
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: {
          kind: "tool_use",
          tool_use_id: "tu-2",
          tool_name: "Read",
          input: { file_path: "/tmp/foo" },
        },
      },
    ]);

    const tools = state.messages.filter(
      (m): m is ToolCallItem => m.kind === "tool_call",
    );
    expect(tools).toHaveLength(1);
    expect(tools[0].status).toBe("running");
  });

  it("tracks permission-request lifecycle: append → resolve → collapse", () => {
    let state = runEvents([
      {
        type: "request_opened",
        thread_id: "t1",
        turn_id: "turn-1",
        request_id: "req-1",
        request_kind: "tool_permission",
        payload: { tool_name: "Bash", tool_input: { command: "rm -rf /" } },
      },
    ]);

    expect(state.pendingRequestIds).toEqual(["req-1"]);
    const opened = state.messages.find(
      (m): m is PermissionRequestItem => m.kind === "permission_request",
    );
    expect(opened).toBeTruthy();
    expect(opened?.resolution.state).toBe("pending");

    state = markRequestResponding(state, "req-1", { decision: "allow" });
    const responding = state.messages.find(
      (m): m is PermissionRequestItem => m.kind === "permission_request",
    );
    expect(responding?.resolution.state).toBe("responding");

    state = applyEvent(state, {
      type: "request_resolved",
      thread_id: "t1",
      request_id: "req-1",
      decision: { decision: "allow" },
    });

    const resolved = state.messages.find(
      (m): m is PermissionRequestItem => m.kind === "permission_request",
    );
    expect(resolved?.resolution.state).toBe("resolved");
    expect(state.pendingRequestIds).toEqual([]);
  });

  it("passes through unknown event variants and warns once", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const base = createEmptyThreadState();

    const weird = {
      type: "totally_unexpected",
      thread_id: "t1",
    } as unknown as ProviderRuntimeEvent;
    const after = applyEvent(base, weird);

    expect(after).toEqual(base);
    // Second call with same variant shouldn't double-warn.
    applyEvent(base, weird);
    expect(warn).toHaveBeenCalledTimes(1);

    warn.mockRestore();
  });

  it("session_state_changed ready/closed/error clears streaming", () => {
    const base: ChatThreadState = { ...createEmptyThreadState(), streaming: true };

    const afterReady = applyEvent(base, {
      type: "session_state_changed",
      thread_id: "t1",
      status: { status: "ready" },
    });
    expect(afterReady.streaming).toBe(false);

    const afterClosed = applyEvent(base, {
      type: "session_state_changed",
      thread_id: "t1",
      status: { status: "closed" },
    });
    expect(afterClosed.streaming).toBe(false);

    const afterError = applyEvent(base, {
      type: "session_state_changed",
      thread_id: "t1",
      status: { status: "error", message: "boom" },
    });
    expect(afterError.streaming).toBe(false);
  });

  it("turn_completed seals the streaming assistant message and clears streaming", () => {
    let state = runEvents([
      {
        type: "session_state_changed",
        thread_id: "t1",
        status: { status: "running", active_turn: "turn-1" },
      },
      {
        type: "content_delta",
        thread_id: "t1",
        turn_id: "turn-1",
        delta: { kind: "text", text: "Hi" },
      },
    ]);

    state = applyEvent(state, {
      type: "turn_completed",
      thread_id: "t1",
      turn_id: "turn-1",
      status: { kind: "success" },
      usage: null,
    });

    const assistant = state.messages.find(
      (m): m is AssistantMessageItem => m.kind === "assistant_message",
    );
    expect(assistant?.streaming).toBe(false);
    expect(state.streaming).toBe(false);
  });

  it("appendUserMessage adds a right-aligned user item", () => {
    const state = appendUserMessage(createEmptyThreadState(), "hello");
    expect(state.messages).toHaveLength(1);
    expect(state.messages[0].kind).toBe("user_message");
  });

  it("resume_cursor_updated is a transcript-level no-op (cursor stored in store, not ChatThreadState)", () => {
    const base = createEmptyThreadState();
    const after = applyEvent(base, {
      type: "resume_cursor_updated",
      thread_id: "t1",
      resume_cursor: { resume: "deadbeef" },
    });
    expect(after).toBe(base);
    expect(after.messages).toHaveLength(0);
  });

  it("seq increases monotonically across alternating user + assistant items", () => {
    // Sequence: user submits -> assistant streams via content_delta ->
    // user submits again -> assistant streams again. Each new item
    // should receive a strictly-greater seq, matching insertion order.
    let state = createEmptyThreadState();
    state = appendUserMessage(state, "first user");
    state = applyEvent(state, {
      type: "content_delta",
      thread_id: "t1",
      turn_id: "turn-1",
      delta: { kind: "text", text: "first reply" },
    });
    state = appendUserMessage(state, "second user");
    state = applyEvent(state, {
      type: "content_delta",
      thread_id: "t1",
      turn_id: "turn-2",
      delta: { kind: "text", text: "second reply" },
    });

    expect(state.messages).toHaveLength(4);
    const seqs = state.messages.map((m) => m.seq);
    expect(seqs).toEqual([0, 1, 2, 3]);
    // Kinds in insertion order: user, assistant, user, assistant.
    expect(state.messages.map((m) => m.kind)).toEqual([
      "user_message",
      "assistant_message",
      "user_message",
      "assistant_message",
    ]);
    // nextSeq keeps advancing for the next append.
    expect(state.nextSeq).toBe(4);
  });

  it("mutating an existing assistant message does not change its seq", () => {
    // A user message followed by two content_deltas on the same turn.
    // The first delta CREATES the assistant item (assigns seq); the
    // second delta MUTATES it (text += delta) and must preserve seq.
    let state = createEmptyThreadState();
    state = appendUserMessage(state, "hello");
    const userSeq = state.messages[0].seq;
    state = applyEvent(state, {
      type: "content_delta",
      thread_id: "t1",
      turn_id: "turn-1",
      delta: { kind: "text", text: "Hi" },
    });
    const assistantSeqFirst = state.messages[1].seq;
    state = applyEvent(state, {
      type: "content_delta",
      thread_id: "t1",
      turn_id: "turn-1",
      delta: { kind: "text", text: " there" },
    });
    expect(state.messages).toHaveLength(2);
    const assistantItem = state.messages[1] as AssistantMessageItem;
    expect(assistantItem.text).toBe("Hi there");
    // seq is the same before and after the second delta.
    expect(assistantItem.seq).toBe(assistantSeqFirst);
    // And strictly greater than the user's seq.
    expect(assistantItem.seq).toBeGreaterThan(userSeq);
    // nextSeq bumped exactly once across both deltas (first created,
    // second mutated).
    expect(state.nextSeq).toBe(2);
  });

  it("runtime_warning does not append to messages (surfaces to console only)", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const base = createEmptyThreadState();

    const after = applyEvent(base, {
      type: "runtime_warning",
      thread_id: "t1",
      message: "stream_event message_start",
      original_payload: { type: "stream_event" },
    });

    expect(after.messages).toHaveLength(0);
    expect(after).toBe(base);
    expect(warn).toHaveBeenCalledWith(
      "[agent-chat]",
      "stream_event message_start",
      { type: "stream_event" },
    );

    warn.mockRestore();
  });
});
