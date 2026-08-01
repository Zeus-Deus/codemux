import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ProviderRuntimeEvent } from "@/tauri/events";

import {
  __resetReducerIdCounterForTests,
  applyEvent,
  appendUserMessage,
  createEmptyThreadState,
  markRequestResponding,
  removeUserMessageByNonce,
  type Clock,
} from "./reducer";
import type {
  AssistantMessageItem,
  ChatThreadState,
  PermissionRequestItem,
  ReasoningItem,
  SubagentRunItem,
  ToolCallItem,
  UserMessageItem,
  WorkflowRunItem,
} from "./types";

function runEvents(
  events: ProviderRuntimeEvent[],
  initial: ChatThreadState = createEmptyThreadState(),
  now?: Clock,
): ChatThreadState {
  // NOTE: apply via an arrow, not `reduce(applyEvent)` — Array.reduce would
  // pass the element index as applyEvent's third (clock) argument.
  return events.reduce<ChatThreadState>(
    (state, event) => applyEvent(state, event, now),
    initial,
  );
}

describe("agent-chat reducer", () => {
  beforeEach(() => {
    __resetReducerIdCounterForTests();
  });

  it("stores complete task snapshots without adding transcript rows", () => {
    const state = applyEvent(createEmptyThreadState(), {
      type: "tasks_updated",
      thread_id: "t1",
      tasks: {
        explanation: "Ship the feature",
        tasks: [
          {
            task_id: "1",
            title: "Wire provider events",
            status: "in_progress",
            blocked_by: [],
          },
        ],
      },
    });
    expect(state.tasks?.explanation).toBe("Ship the feature");
    expect(state.tasks?.tasks[0].status).toBe("in_progress");
    expect(state.tasksUpdatedAt).toEqual(expect.any(Number));
    expect(state.messages).toEqual([]);
  });

  it("replaces rather than merges task snapshots", () => {
    const first = applyEvent(createEmptyThreadState(), {
      type: "tasks_updated",
      thread_id: "t1",
      tasks: {
        tasks: [{ task_id: "1", title: "Old", status: "pending", blocked_by: [] }],
      },
    });
    const next = applyEvent(first, {
      type: "tasks_updated",
      thread_id: "t1",
      tasks: {
        tasks: [{ task_id: "2", title: "New", status: "completed", blocked_by: [] }],
      },
    });
    expect(next.tasks?.tasks.map((task) => task.task_id)).toEqual(["2"]);
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

  describe("empty text deltas never materialize an assistant message", () => {
    const textDelta = (text: string): ProviderRuntimeEvent => ({
      type: "content_delta",
      thread_id: "t1",
      turn_id: "turn-1",
      delta: { kind: "text", text },
    });
    const assistants = (s: ChatThreadState): AssistantMessageItem[] =>
      s.messages.filter(
        (m): m is AssistantMessageItem => m.kind === "assistant_message",
      );

    it("an empty first delta creates no assistant item", () => {
      const state = runEvents([textDelta("")]);
      expect(assistants(state)).toHaveLength(0);
      // Nothing appended → nextSeq untouched.
      expect(state.nextSeq).toBe(0);
      // The delta still marks the turn streaming (session-owned flag path).
      expect(state.streaming).toBe(true);
    });

    it("a whitespace-only first delta creates no assistant item", () => {
      const state = runEvents([textDelta("   \n\t")]);
      expect(assistants(state)).toHaveLength(0);
      expect(state.nextSeq).toBe(0);
    });

    it("an empty delta then a non-empty delta yields exactly one item with the right text", () => {
      const state = runEvents([textDelta(""), textDelta("Hello")]);
      const list = assistants(state);
      expect(list).toHaveLength(1);
      expect(list[0].text).toBe("Hello");
      expect(list[0].streaming).toBe(true);
    });

    it("an empty delta merging into an existing streaming tail is a no-op (same reference, same text)", () => {
      const first = runEvents([textDelta("Hi")]);
      const before = first.messages[first.messages.length - 1];
      const after = applyEvent(first, textDelta(""));
      const list = assistants(after);
      expect(list).toHaveLength(1);
      expect(list[0].text).toBe("Hi");
      // Reference-stable: the empty merge cloned nothing.
      expect(after.messages[after.messages.length - 1]).toBe(before);
    });

    it("an empty delta between tool steps keeps the run contiguous (no phantom item to break it)", () => {
      // Reproduces the settle-mid-run bug: a Read runs, then Claude opens a
      // new text block with an empty delta, then another Read runs. The
      // empty delta must NOT land an assistant_message between the two
      // tools (which would flush the live Activity run as settled).
      const state = runEvents([
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: {
            kind: "tool_use",
            tool_use_id: "tu-1",
            tool_name: "Read",
            input: { file_path: "/a" },
          },
        },
        textDelta(""),
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: {
            kind: "tool_use",
            tool_use_id: "tu-2",
            tool_name: "Read",
            input: { file_path: "/b" },
          },
        },
      ]);
      expect(assistants(state)).toHaveLength(0);
      expect(state.messages.map((m) => m.kind)).toEqual([
        "tool_call",
        "tool_call",
      ]);
    });

    it("an assistant_text completion after only-empty deltas settles without a blank row", () => {
      const state = runEvents([
        {
          type: "session_state_changed",
          thread_id: "t1",
          status: { status: "running", active_turn: "turn-1" },
        },
        textDelta(""),
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: { kind: "assistant_text", text: "" },
        },
        {
          type: "turn_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          status: { kind: "success" },
          usage: null,
        },
      ]);
      // No orphaned/blank assistant row, no crash, turn settled.
      expect(assistants(state)).toHaveLength(0);
      expect(state.streaming).toBe(false);
    });

    it("hydrate/replay of the same deltas produces an identical item count", () => {
      const events: ProviderRuntimeEvent[] = [
        textDelta(""),
        textDelta("real text"),
        textDelta(""),
      ];
      const live = runEvents(events);
      __resetReducerIdCounterForTests();
      const replayed = runEvents(events);
      expect(assistants(replayed).length).toBe(assistants(live).length);
      expect(assistants(replayed)[0].text).toBe("real text");
    });
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

  it("stamps started_at on tool_use and completed_at on tool_result from the injected clock", () => {
    // Clock ticks 100 → 100 → 200: use lands at 100, result at 200.
    const ticks = [100, 200];
    let i = 0;
    const clock: Clock = () => ticks[Math.min(i++, ticks.length - 1)];
    const state = runEvents(
      [
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: {
            kind: "tool_use",
            tool_use_id: "tu-9",
            tool_name: "Bash",
            input: { command: "cargo test" },
          },
        },
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: {
            kind: "tool_result",
            tool_use_id: "tu-9",
            content: "ok",
            is_error: false,
          },
        },
      ],
      createEmptyThreadState(),
      clock,
    );
    const tool = state.messages.find(
      (m): m is ToolCallItem => m.kind === "tool_call",
    );
    expect(tool?.started_at).toBe(100);
    expect(tool?.completed_at).toBe(200);
  });

  it("stamps completed_at on a result-first placeholder and started_at when the use lands", () => {
    const ticks = [500, 900];
    let i = 0;
    const clock: Clock = () => ticks[Math.min(i++, ticks.length - 1)];
    const state = runEvents(
      [
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: { kind: "tool_result", tool_use_id: "tu-r", content: "42", is_error: false },
        },
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: {
            kind: "tool_use",
            tool_use_id: "tu-r",
            tool_name: "Bash",
            input: { command: "echo 42" },
          },
        },
      ],
      createEmptyThreadState(),
      clock,
    );
    const tool = state.messages.find(
      (m): m is ToolCallItem => m.kind === "tool_call",
    );
    // Result landed first (500), then the use stamped its own start (900).
    expect(tool?.completed_at).toBe(500);
    expect(tool?.started_at).toBe(900);
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
        tool_use_id: null,
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

  it("keeps a resolved request resolved when a duplicate respond fails", () => {
    // Two windows on the same thread: the second respond loses the race and
    // the provider answers "not found" — which it cannot distinguish from
    // "already resolved" (Codex drops the pending entry on the first answer).
    // The persisted failure event must not flip an answered approval into
    // the expired state, live or on replay.
    let state = runEvents([
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: {
          kind: "tool_use",
          tool_name: "Bash",
          input: { command: "pwd" },
          tool_use_id: "tool-dup",
        },
      },
      {
        type: "request_opened",
        thread_id: "t1",
        turn_id: "turn-1",
        request_id: "req-dup",
        request_kind: "tool-permission",
        payload: { tool_name: "Bash" },
        tool_use_id: "tool-dup",
      },
      {
        type: "request_resolved",
        thread_id: "t1",
        request_id: "req-dup",
        decision: { decision: "allow" },
      },
    ]);

    state = applyEvent(state, {
      type: "request_response_failed",
      thread_id: "t1",
      request_id: "req-dup",
      reason: "stale_provider_callback",
      message: "Question expired.",
    });

    const request = state.messages.find(
      (m): m is PermissionRequestItem =>
        m.kind === "permission_request" && m.request_id === "req-dup",
    );
    expect(request?.resolution).toEqual({
      state: "resolved",
      decision: { decision: "allow" },
    });
    // The allowed tool keeps running — the failure belongs to a duplicate
    // reply, not to the tool call the user approved.
    const tool = state.messages.find(
      (m) => m.kind === "tool_call" && m.tool_use_id === "tool-dup",
    );
    expect(tool?.kind === "tool_call" && tool.status).toBe("running");
    expect(state.pendingRequestIds).toEqual([]);
  });

  it("terminalizes a stale request response and settles its linked tool", () => {
    let state = runEvents([
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: {
          kind: "tool_use",
          tool_name: "Bash",
          input: { command: "pwd" },
          tool_use_id: "tool-1",
        },
      },
      {
        type: "request_opened",
        thread_id: "t1",
        turn_id: "turn-1",
        request_id: "req-stale",
        request_kind: "tool-permission",
        payload: { tool_name: "Bash" },
        tool_use_id: "tool-1",
      },
    ]);

    state = applyEvent(state, {
      type: "request_response_failed",
      thread_id: "t1",
      request_id: "req-stale",
      reason: "stale_provider_callback",
      message: "Question expired.",
    });

    const request = state.messages.find(
      (m): m is PermissionRequestItem =>
        m.kind === "permission_request" && m.request_id === "req-stale",
    );
    expect(request?.resolution).toEqual({
      state: "failed",
      reason: "stale_provider_callback",
      message: "Question expired.",
    });
    const tool = state.messages.find(
      (m) => m.kind === "tool_call" && m.tool_use_id === "tool-1",
    );
    expect(tool?.kind === "tool_call" && tool.status).toBe("error");
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

  it("appendUserMessage attaches images when provided", () => {
    const state = appendUserMessage(
      createEmptyThreadState(),
      "look at this",
      undefined,
      undefined,
      [{ src: "data:image/png;base64,AAAA", mediaType: "image/png" }],
    );
    const item = state.messages[0];
    expect(item.kind).toBe("user_message");
    if (item.kind === "user_message") {
      expect(item.images).toEqual([
        { src: "data:image/png;base64,AAAA", mediaType: "image/png" },
      ]);
    }
  });

  it("appendUserMessage omits `images` entirely for a text-only turn", () => {
    // Empty / undefined image lists must not stamp an `images` key, so
    // a text-only bubble stays byte-identical to the pre-images shape.
    const withUndefined = appendUserMessage(createEmptyThreadState(), "hi");
    const withEmpty = appendUserMessage(
      createEmptyThreadState(),
      "hi",
      undefined,
      undefined,
      [],
    );
    expect("images" in withUndefined.messages[0]).toBe(false);
    expect("images" in withEmpty.messages[0]).toBe(false);
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

  it("links a permission_request to an existing tool_call when tool_use_id matches", () => {
    // Stage 1 wire change: request_opened carries tool_use_id so the
    // reducer can stamp the corresponding ToolCallItem with
    // approval_request_id, which the ToolCallCard renderer reads to
    // show its inline approval footer.
    let state = runEvents([
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: {
          kind: "tool_use",
          tool_use_id: "tu-match",
          tool_name: "Bash",
          input: { command: "ls" },
        },
      },
    ]);
    state = applyEvent(state, {
      type: "request_opened",
      thread_id: "t1",
      turn_id: "turn-1",
      request_id: "req-bash",
      request_kind: "command",
      payload: { tool_name: "Bash", tool_input: { command: "ls" } },
      tool_use_id: "tu-match",
    });

    const tool = state.messages.find(
      (m): m is ToolCallItem => m.kind === "tool_call",
    );
    expect(tool?.approval_request_id).toBe("req-bash");
    const req = state.messages.find(
      (m): m is PermissionRequestItem => m.kind === "permission_request",
    );
    expect(req?.tool_use_id).toBe("tu-match");
  });

  it("links a tool_call to a permission_request that arrived first", () => {
    // Defensive path: if request_opened lands before the assistant's
    // tool_use block (uncommon but possible), the reducer stamps the
    // link when the tool_use eventually arrives.
    let state = applyEvent(createEmptyThreadState(), {
      type: "request_opened",
      thread_id: "t1",
      turn_id: "turn-1",
      request_id: "req-early",
      request_kind: "command",
      payload: { tool_name: "Bash" },
      tool_use_id: "tu-late",
    });
    state = applyEvent(state, {
      type: "item_completed",
      thread_id: "t1",
      turn_id: "turn-1",
      item: {
        kind: "tool_use",
        tool_use_id: "tu-late",
        tool_name: "Bash",
        input: { command: "ls" },
      },
    });

    const tool = state.messages.find(
      (m): m is ToolCallItem => m.kind === "tool_call",
    );
    expect(tool?.approval_request_id).toBe("req-early");
  });

  it("leaves approval_request_id null when no tool_use_id matches", () => {
    // Standalone permission requests (plan, user-input, or tool
    // requests where the tool_use hasn't been seen) don't stamp any
    // ToolCallItem — the row continues to render via the generic
    // PermissionRequestBlock fallback.
    const state = applyEvent(createEmptyThreadState(), {
      type: "request_opened",
      thread_id: "t1",
      turn_id: "turn-1",
      request_id: "req-plan",
      request_kind: "plan",
      payload: "markdown body",
      tool_use_id: null,
    });
    const tools = state.messages.filter(
      (m): m is ToolCallItem => m.kind === "tool_call",
    );
    expect(tools).toHaveLength(0);
    const req = state.messages.find(
      (m): m is PermissionRequestItem => m.kind === "permission_request",
    );
    expect(req?.tool_use_id).toBeNull();
  });

  describe("Stage 3 fix — specialized tools do not create ToolCallItems", () => {
    it("ExitPlanMode tool_use does NOT append a ToolCallItem", () => {
      const state = runEvents([
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: {
            kind: "tool_use",
            tool_use_id: "tu-plan-1",
            tool_name: "ExitPlanMode",
            input: { plan: "# Refactor\n\n- Step one" },
          },
        },
      ]);
      const tools = state.messages.filter(
        (m): m is ToolCallItem => m.kind === "tool_call",
      );
      expect(tools).toHaveLength(0);
      // nextSeq untouched because no item was appended.
      expect(state.nextSeq).toBe(0);
    });

    it("AskUserQuestion tool_use does NOT append a ToolCallItem", () => {
      const state = runEvents([
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: {
            kind: "tool_use",
            tool_use_id: "tu-ask-1",
            tool_name: "AskUserQuestion",
            input: { questions: [] },
          },
        },
      ]);
      expect(
        state.messages.filter((m) => m.kind === "tool_call"),
      ).toHaveLength(0);
      expect(state.nextSeq).toBe(0);
    });

    it("ExitPlanMode tool_use followed by request_opened(plan) leaves only the PermissionRequestItem", () => {
      // Matches the real sidecar ordering: assistant tool_use block
      // lands first, then the plan-proposed notification.
      const state = runEvents([
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: {
            kind: "tool_use",
            tool_use_id: "tu-plan-2",
            tool_name: "ExitPlanMode",
            input: { plan: "# Refactor" },
          },
        },
        {
          type: "request_opened",
          thread_id: "t1",
          turn_id: "turn-1",
          request_id: "tu-plan-2", // sidecar uses tool_use_id as request_id for plan
          request_kind: "plan",
          payload: { plan: "# Refactor" },
          tool_use_id: "tu-plan-2",
        },
      ]);
      expect(
        state.messages.filter((m) => m.kind === "tool_call"),
      ).toHaveLength(0);
      const req = state.messages.find(
        (m): m is PermissionRequestItem => m.kind === "permission_request",
      );
      expect(req?.request_kind).toBe("plan");
      // Nothing should have claimed ownership of this request — the
      // mergedRequestIds selector in MessageList would otherwise
      // swallow it into a phantom ToolCallCard.
      expect(req?.tool_use_id).toBe("tu-plan-2");
    });

    it("request_opened(plan) before the ExitPlanMode tool_use still yields only the PermissionRequestItem", () => {
      // Reverse ordering — tests both guards (tool_use skip AND the
      // tool_use branch's priorRequest association skipping for the
      // specialized path).
      const state = runEvents([
        {
          type: "request_opened",
          thread_id: "t1",
          turn_id: "turn-1",
          request_id: "tu-plan-3",
          request_kind: "plan",
          payload: { plan: "# Refactor" },
          tool_use_id: "tu-plan-3",
        },
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: {
            kind: "tool_use",
            tool_use_id: "tu-plan-3",
            tool_name: "ExitPlanMode",
            input: { plan: "# Refactor" },
          },
        },
      ]);
      expect(
        state.messages.filter((m) => m.kind === "tool_call"),
      ).toHaveLength(0);
      expect(
        state.messages.filter((m) => m.kind === "permission_request"),
      ).toHaveLength(1);
    });

    it("AskUserQuestion tool_use + request_opened(user-input) same pair — no ToolCallItem, request stands alone", () => {
      const state = runEvents([
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: {
            kind: "tool_use",
            tool_use_id: "tu-ask-2",
            tool_name: "AskUserQuestion",
            input: { questions: [] },
          },
        },
        {
          type: "request_opened",
          thread_id: "t1",
          turn_id: "turn-1",
          request_id: "req-ask-2",
          request_kind: "user-input",
          payload: { questions: [] },
          tool_use_id: "tu-ask-2",
        },
      ]);
      expect(
        state.messages.filter((m) => m.kind === "tool_call"),
      ).toHaveLength(0);
      const req = state.messages.find(
        (m): m is PermissionRequestItem => m.kind === "permission_request",
      );
      expect(req?.request_kind).toBe("user-input");
    });

    it("ExitPlanMode tool_result that somehow lands does NOT create a ghost placeholder when a plan request is pending", () => {
      // Defensive: in practice the sidecar denies ExitPlanMode before
      // a tool_result is emitted. But if a future SDK change leaks
      // one through, the tool_use_id will match a pending plan
      // request — skip the placeholder so the PlanProposalBlock
      // stays clean.
      const state = runEvents([
        {
          type: "request_opened",
          thread_id: "t1",
          turn_id: "turn-1",
          request_id: "tu-plan-4",
          request_kind: "plan",
          payload: { plan: "# Refactor" },
          tool_use_id: "tu-plan-4",
        },
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: {
            kind: "tool_result",
            tool_use_id: "tu-plan-4",
            content: "ignored",
            is_error: false,
          },
        },
      ]);
      expect(
        state.messages.filter((m) => m.kind === "tool_call"),
      ).toHaveLength(0);
    });

    it("regression guard — Bash tool_use still creates a ToolCallItem", () => {
      // Ensures the allowlist is strict (not a blanket skip) and
      // regular tools keep flowing through the Stage 1 merge path.
      const state = runEvents([
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: {
            kind: "tool_use",
            tool_use_id: "tu-bash",
            tool_name: "Bash",
            input: { command: "ls" },
          },
        },
      ]);
      const tools = state.messages.filter(
        (m): m is ToolCallItem => m.kind === "tool_call",
      );
      expect(tools).toHaveLength(1);
      expect(tools[0].tool_name).toBe("Bash");
    });
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

  it("runtime_warning promotes a rejected rate-limit event to an inline runtime_notice", () => {
    const state = applyEvent(createEmptyThreadState(), {
      type: "runtime_warning",
      thread_id: "t1",
      message: "rate limit event",
      original_payload: { rate_limit_info: { status: "rejected" } },
    });
    expect(state.messages).toHaveLength(1);
    const notice = state.messages[0];
    expect(notice.kind).toBe("runtime_notice");
    if (notice.kind === "runtime_notice") {
      expect(notice.message).toBe(
        "Usage limit reached — the provider stopped the run.",
      );
    }
  });

  it("runtime_warning debug noise still appends nothing", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const base = createEmptyThreadState();
    const after = applyEvent(base, {
      type: "runtime_warning",
      thread_id: "t1",
      message: "rate limit event",
      original_payload: { rate_limit_info: { status: "allowed" } },
    });
    expect(after).toBe(base);
    warn.mockRestore();
  });

  it("content_delta that follows a sealed assistant + tool_call starts a NEW assistant block at a fresh seq (AskUserQuestion position bug)", () => {
    // Reproduces the AskUserQuestion "Answered above older tool calls"
    // bug. Timeline: assistant emits a preamble, a tool runs, the user
    // answers an AskUserQuestion, and then the assistant continues
    // with a new text block. That continuation must land AFTER the
    // tool call, not get merged back into the first (sealed) assistant
    // at its lower seq — otherwise the new prose visually leapfrogs
    // every intervening tool/approval marker.
    const state = runEvents([
      {
        type: "session_state_changed",
        thread_id: "t1",
        status: { status: "running", active_turn: "turn-1" },
      },
      // Preamble: stream a text delta then seal it.
      {
        type: "content_delta",
        thread_id: "t1",
        turn_id: "turn-1",
        delta: { kind: "text", text: "Checking the repo" },
      },
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: { kind: "assistant_text", text: "Checking the repo" },
      },
      // A tool call.
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: {
          kind: "tool_use",
          tool_use_id: "tu-1",
          tool_name: "Read",
          input: { path: "README.md" },
        },
      },
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: {
          kind: "tool_result",
          tool_use_id: "tu-1",
          content: "ok",
          is_error: false,
        },
      },
      // An AskUserQuestion approval, resolved by the user's answer.
      {
        type: "request_opened",
        thread_id: "t1",
        turn_id: "turn-1",
        request_id: "req-uq-1",
        request_kind: "user-input",
        payload: {},
        tool_use_id: "tu-uq-1",
      },
      {
        type: "request_resolved",
        thread_id: "t1",
        request_id: "req-uq-1",
        decision: { decision: "allow" },
      },
      // Assistant continues with a brand-new text block after the
      // answer. This is where the bug used to bite.
      {
        type: "content_delta",
        thread_id: "t1",
        turn_id: "turn-1",
        delta: { kind: "text", text: "Got it — writing the README." },
      },
    ]);

    const assistants = state.messages.filter(
      (m): m is AssistantMessageItem => m.kind === "assistant_message",
    );
    expect(assistants).toHaveLength(2);
    expect(assistants[0].text).toBe("Checking the repo");
    expect(assistants[0].streaming).toBe(false);
    expect(assistants[1].text).toBe("Got it — writing the README.");
    expect(assistants[1].streaming).toBe(true);

    // Chronological render order: preamble → tool → Answered → new prose.
    const kinds = state.messages
      .slice()
      .sort((a, b) => a.seq - b.seq)
      .map((m) => m.kind);
    expect(kinds).toEqual([
      "assistant_message",
      "tool_call",
      "permission_request",
      "assistant_message",
    ]);
    // And the new block must sit at a seq STRICTLY GREATER than every
    // prior item, not back-merged onto the sealed preamble.
    expect(assistants[1].seq).toBeGreaterThan(assistants[0].seq);
    const permReq = state.messages.find(
      (m) => m.kind === "permission_request",
    )!;
    expect(assistants[1].seq).toBeGreaterThan(permReq.seq);
  });

  it("caps message history once the thread quiets between turns", () => {
    // Long-running threads accumulate transcript items forever. Without
    // a cap, every reducer event spreads into a growing array and the
    // React fan-out scales with item count, eventually stalling the
    // main thread. The cap kicks in only between turns (not streaming,
    // no pending requests) so mid-turn correlation lookups against
    // earlier `tool_use_id` / `request_id` rows can't be orphaned.
    let state = createEmptyThreadState();
    // 5500 sealed user/assistant pairs — well beyond MAX_MESSAGES_PER_THREAD.
    for (let i = 0; i < 5500; i += 1) {
      state = appendUserMessage(state, `msg-${i}`);
    }

    expect(state.messages.length).toBeLessThanOrEqual(5_000);
    // Trim should drop the oldest items, not the most recent ones.
    const lastFive = state.messages.slice(-5).map((m) => {
      if (m.kind !== "user_message") {
        throw new Error("expected user_message");
      }
      return m.text;
    });
    expect(lastFive).toEqual([
      "msg-5495",
      "msg-5496",
      "msg-5497",
      "msg-5498",
      "msg-5499",
    ]);
  });

  it("does not trim mid-stream or while approvals are pending", () => {
    // Mid-turn drops would orphan tool_use_id / request_id correlation
    // (`findToolCallByUseId`, `findPermissionRequest`) that scan the
    // full array — the guard must hold even when the array is huge.
    let state: ChatThreadState = {
      ...createEmptyThreadState(),
      streaming: true,
    };
    for (let i = 0; i < 5500; i += 1) {
      state = appendUserMessage(state, `msg-${i}`);
    }
    expect(state.streaming).toBe(true);
    expect(state.messages.length).toBe(5_500);

    state = {
      ...createEmptyThreadState(),
      pendingRequestIds: ["req-1"],
    };
    for (let i = 0; i < 5500; i += 1) {
      state = appendUserMessage(state, `msg-${i}`);
    }
    expect(state.pendingRequestIds).toEqual(["req-1"]);
    expect(state.messages.length).toBe(5_500);
  });

  describe("reasoning items (D5 thinking blocks)", () => {
    const thinkingDelta = (
      turn: string,
      text: string,
    ): ProviderRuntimeEvent => ({
      type: "content_delta",
      thread_id: "t1",
      turn_id: turn,
      delta: { kind: "thinking", text },
    });
    const thinkingDone = (turn: string, text: string): ProviderRuntimeEvent => ({
      type: "item_completed",
      thread_id: "t1",
      turn_id: turn,
      item: { kind: "assistant_thinking", text },
    });
    const textDelta = (turn: string, text: string): ProviderRuntimeEvent => ({
      type: "content_delta",
      thread_id: "t1",
      turn_id: turn,
      delta: { kind: "text", text },
    });
    const toolUse = (
      turn: string,
      id: string,
      name = "Read",
    ): ProviderRuntimeEvent => ({
      type: "item_completed",
      thread_id: "t1",
      turn_id: turn,
      item: { kind: "tool_use", tool_use_id: id, tool_name: name, input: {} },
    });
    const reasoningItems = (state: ChatThreadState): ReasoningItem[] =>
      state.messages.filter((m): m is ReasoningItem => m.kind === "reasoning");

    it("accumulates thinking deltas into one streaming reasoning item", () => {
      const state = runEvents(
        [
          {
            type: "session_state_changed",
            thread_id: "t1",
            status: { status: "running", active_turn: "turn-1" },
          },
          thinkingDelta("turn-1", "First"),
          thinkingDelta("turn-1", " then more"),
        ],
        undefined,
        () => 1234,
      );
      const reasoning = reasoningItems(state);
      expect(reasoning).toHaveLength(1);
      expect(reasoning[0].text).toBe("First then more");
      expect(reasoning[0].streaming).toBe(true);
      expect(reasoning[0].started_at).toBe(1234);
      expect(reasoning[0].duration_ms).toBeUndefined();
      expect(state.streaming).toBe(true);
    });

    it("finalizes on assistant_thinking: replaces text, seals, and records duration first-delta→completion", () => {
      let t = 0;
      const clock: Clock = () => t;
      let state = createEmptyThreadState();
      t = 1000;
      state = applyEvent(state, thinkingDelta("turn-1", "Let me "), clock);
      t = 3000; // second delta appends — must NOT reset started_at
      state = applyEvent(
        state,
        thinkingDelta("turn-1", "inspect the file."),
        clock,
      );
      t = 7000; // completion → duration = 7000 - 1000
      state = applyEvent(
        state,
        thinkingDone("turn-1", "Let me inspect the file."),
        clock,
      );
      const reasoning = reasoningItems(state);
      expect(reasoning).toHaveLength(1);
      expect(reasoning[0].text).toBe("Let me inspect the file.");
      expect(reasoning[0].streaming).toBe(false);
      expect(reasoning[0].started_at).toBe(1000);
      expect(reasoning[0].duration_ms).toBe(6000);
    });

    it("materialises a sealed reasoning item from a completion that carried no deltas (no duration)", () => {
      const state = runEvents([thinkingDone("turn-1", "Instant thought")]);
      const reasoning = reasoningItems(state);
      expect(reasoning).toHaveLength(1);
      expect(reasoning[0].text).toBe("Instant thought");
      expect(reasoning[0].streaming).toBe(false);
      expect(reasoning[0].started_at).toBeUndefined();
      expect(reasoning[0].duration_ms).toBeUndefined();
    });

    it("seals a streaming reasoning block when a tool_use lands after it (no completion seen)", () => {
      let t = 100;
      const clock: Clock = () => t;
      let state = createEmptyThreadState();
      state = applyEvent(state, thinkingDelta("turn-1", "pondering"), clock);
      t = 900; // tool_use boundary → duration = 900 - 100
      state = applyEvent(state, toolUse("turn-1", "tu-1"), clock);
      const reasoning = reasoningItems(state);
      expect(reasoning).toHaveLength(1);
      expect(reasoning[0].streaming).toBe(false);
      expect(reasoning[0].duration_ms).toBe(800);
      expect(state.messages.map((m) => m.kind)).toEqual([
        "reasoning",
        "tool_call",
      ]);
    });

    it("a text delta after a streaming reasoning block seals it and starts a fresh assistant row", () => {
      const state = runEvents([
        thinkingDelta("turn-1", "considering"),
        textDelta("turn-1", "Here is the answer"),
      ]);
      const reasoning = reasoningItems(state);
      expect(reasoning[0].streaming).toBe(false);
      const assistants = state.messages.filter(
        (m): m is AssistantMessageItem => m.kind === "assistant_message",
      );
      expect(assistants).toHaveLength(1);
      expect(assistants[0].text).toBe("Here is the answer");
      expect(assistants[0].streaming).toBe(true);
      expect(state.messages.map((m) => m.kind)).toEqual([
        "reasoning",
        "assistant_message",
      ]);
    });

    it("interleaves reasoning, a tool call, and assistant text in chronological seq order", () => {
      const state = runEvents([
        {
          type: "session_state_changed",
          thread_id: "t1",
          status: { status: "running", active_turn: "turn-1" },
        },
        thinkingDelta("turn-1", "checking"),
        thinkingDone("turn-1", "checking the repo"),
        toolUse("turn-1", "tu-1"),
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: {
            kind: "tool_result",
            tool_use_id: "tu-1",
            content: "ok",
            is_error: false,
          },
        },
        {
          type: "item_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          item: { kind: "assistant_text", text: "Found it" },
        },
      ]);
      const ordered = state.messages
        .slice()
        .sort((a, b) => a.seq - b.seq)
        .map((m) => m.kind);
      expect(ordered).toEqual([
        "reasoning",
        "tool_call",
        "assistant_message",
      ]);
      const reasoning = reasoningItems(state);
      expect(reasoning[0].text).toBe("checking the repo");
      expect(reasoning[0].streaming).toBe(false);
    });

    it("starts a NEW reasoning block for a second thinking segment after the first sealed", () => {
      const state = runEvents([
        thinkingDelta("turn-1", "first"),
        thinkingDone("turn-1", "first"),
        thinkingDelta("turn-1", "second"),
        thinkingDone("turn-1", "second"),
      ]);
      const reasoning = reasoningItems(state);
      expect(reasoning).toHaveLength(2);
      expect(reasoning.map((r) => r.text)).toEqual(["first", "second"]);
      expect(reasoning.every((r) => !r.streaming)).toBe(true);
    });

    it("preserves the reasoning seq across delta accumulation (mutation keeps seq)", () => {
      let state = createEmptyThreadState();
      state = appendUserMessage(state, "go");
      state = applyEvent(state, thinkingDelta("turn-1", "a"));
      const firstSeq = state.messages[1].seq;
      state = applyEvent(state, thinkingDelta("turn-1", "b"));
      const reasoning = state.messages[1] as ReasoningItem;
      expect(reasoning.kind).toBe("reasoning");
      expect(reasoning.text).toBe("ab");
      expect(reasoning.seq).toBe(firstSeq);
      // user(0) + reasoning(1); the second delta MUTATED, so nextSeq === 2.
      expect(state.nextSeq).toBe(2);
    });

    it("turn_completed seals a still-streaming reasoning block", () => {
      let t = 10;
      const clock: Clock = () => t;
      let state = createEmptyThreadState();
      state = applyEvent(state, thinkingDelta("turn-1", "mid-thought"), clock);
      t = 40;
      state = applyEvent(
        state,
        {
          type: "turn_completed",
          thread_id: "t1",
          turn_id: "turn-1",
          status: { kind: "success" },
          usage: null,
        },
        clock,
      );
      const reasoning = reasoningItems(state);
      expect(reasoning[0].streaming).toBe(false);
      expect(reasoning[0].duration_ms).toBe(30);
      expect(state.streaming).toBe(false);
    });

    it("subjects reasoning items to the 5,000-message cap like any other row", () => {
      let state = createEmptyThreadState();
      // A reasoning block near the head, then quiesce so the cap can fire.
      state = applyEvent(state, thinkingDelta("turn-1", "head thought"));
      state = applyEvent(state, thinkingDone("turn-1", "head thought"));
      state = applyEvent(state, {
        type: "turn_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        status: { kind: "success" },
        usage: null,
      });
      expect(reasoningItems(state)).toHaveLength(1);
      for (let i = 0; i < 5200; i += 1) {
        state = appendUserMessage(state, `m-${i}`);
      }
      expect(state.messages.length).toBeLessThanOrEqual(5_000);
      // The head reasoning block was trimmed with the rest — no special-casing.
      expect(reasoningItems(state)).toHaveLength(0);
    });
  });
});

describe("agent-chat reducer — follow-up queueing", () => {
  beforeEach(() => {
    __resetReducerIdCounterForTests();
  });

  const users = (s: ChatThreadState): UserMessageItem[] =>
    s.messages.filter((m): m is UserMessageItem => m.kind === "user_message");

  it("turn_queued reconciles the optimistic bubble by client nonce (no duplicate)", () => {
    // Composer optimistically appended a bubble carrying a nonce.
    let state = appendUserMessage(
      createEmptyThreadState(),
      "second message",
      undefined,
      "nonce-x",
    );
    expect(users(state)).toHaveLength(1);
    expect(users(state)[0].queued).toBeUndefined();

    state = applyEvent(state, {
      type: "turn_queued",
      thread_id: "t1",
      queued_id: "q-1",
      client_nonce: "nonce-x",
      text: "second message",
    });

    const list = users(state);
    expect(list).toHaveLength(1); // reconciled, not duplicated
    expect(list[0].queued).toEqual({ queuedId: "q-1" });
    expect(list[0].text).toBe("second message");
  });

  it("turn_queued without a matching bubble appends a greyed item (remount case)", () => {
    const state = applyEvent(createEmptyThreadState(), {
      type: "turn_queued",
      thread_id: "t1",
      queued_id: "q-2",
      client_nonce: null,
      text: "reconstructed",
    });
    const list = users(state);
    expect(list).toHaveLength(1);
    expect(list[0].queued).toEqual({ queuedId: "q-2" });
    expect(list[0].text).toBe("reconstructed");
  });

  it("queued items sort to the very bottom, below later streaming content", () => {
    let state = appendUserMessage(
      createEmptyThreadState(),
      "queued one",
      undefined,
      "n1",
    );
    state = applyEvent(state, {
      type: "turn_queued",
      thread_id: "t1",
      queued_id: "q-1",
      client_nonce: "n1",
      text: "queued one",
    });
    // An assistant message streams in AFTER the enqueue.
    state = applyEvent(state, {
      type: "content_delta",
      thread_id: "t1",
      turn_id: "turn-1",
      delta: { kind: "text", text: "assistant reply" },
    });
    const queued = users(state)[0];
    const assistant = state.messages.find(
      (m): m is AssistantMessageItem => m.kind === "assistant_message",
    );
    expect(queued.seq).toBeGreaterThan(assistant!.seq);
  });

  it("queued_turn_dispatched promotes the bubble and re-seqs it to the tail", () => {
    let state = appendUserMessage(
      createEmptyThreadState(),
      "hello",
      undefined,
      "n1",
    );
    state = applyEvent(state, {
      type: "turn_queued",
      thread_id: "t1",
      queued_id: "q-1",
      client_nonce: "n1",
      text: "hello",
    });
    const queuedSeq = users(state)[0].seq;
    state = applyEvent(state, {
      type: "queued_turn_dispatched",
      thread_id: "t1",
      queued_id: "q-1",
      turn_id: "turn-9",
      text: "hello",
    });
    const list = users(state);
    expect(list).toHaveLength(1);
    expect(list[0].queued).toBeUndefined(); // promoted to normal
    expect(list[0].seq).toBeLessThan(queuedSeq); // re-seq'd out of the queued band
    expect(list[0].text).toBe("hello");
  });

  it("queued_turn_cancelled removes the greyed bubble", () => {
    let state = applyEvent(createEmptyThreadState(), {
      type: "turn_queued",
      thread_id: "t1",
      queued_id: "q-3",
      client_nonce: null,
      text: "bye",
    });
    expect(users(state)).toHaveLength(1);
    state = applyEvent(state, {
      type: "queued_turn_cancelled",
      thread_id: "t1",
      queued_id: "q-3",
    });
    expect(users(state)).toHaveLength(0);
  });

  it("dispatch/cancel for an unknown queued id are safe no-ops", () => {
    const base = appendUserMessage(createEmptyThreadState(), "x", undefined, "n");
    const afterDispatch = applyEvent(base, {
      type: "queued_turn_dispatched",
      thread_id: "t1",
      queued_id: "missing",
      turn_id: "t",
      text: "x",
    });
    expect(afterDispatch.messages).toEqual(base.messages);
    const afterCancel = applyEvent(base, {
      type: "queued_turn_cancelled",
      thread_id: "t1",
      queued_id: "missing",
    });
    expect(afterCancel.messages).toEqual(base.messages);
  });

  it("removeUserMessageByNonce rolls back an optimistic bubble", () => {
    const state = appendUserMessage(
      createEmptyThreadState(),
      "oops",
      undefined,
      "nonce-err",
    );
    expect(users(state)).toHaveLength(1);
    const rolledBack = removeUserMessageByNonce(state, "nonce-err");
    expect(users(rolledBack)).toHaveLength(0);
    // Unknown nonce is a no-op.
    expect(removeUserMessageByNonce(state, "other").messages).toEqual(
      state.messages,
    );
  });
});

describe("agent-chat reducer — subagents", () => {
  beforeEach(() => {
    __resetReducerIdCounterForTests();
  });

  function subagentUpdated(
    subagent: Record<string, unknown>,
  ): ProviderRuntimeEvent {
    return {
      type: "subagent_updated",
      thread_id: "t1",
      subagent: subagent as never,
    } as ProviderRuntimeEvent;
  }
  function subItem(
    subagentId: string,
    item: Record<string, unknown>,
  ): ProviderRuntimeEvent {
    return {
      type: "item_completed",
      thread_id: "t1",
      turn_id: "turn-1",
      subagent_id: subagentId,
      item: item as never,
    } as ProviderRuntimeEvent;
  }
  function cards(state: ChatThreadState): SubagentRunItem[] {
    return state.messages.filter(
      (m): m is SubagentRunItem => m.kind === "subagent_run",
    );
  }

  it("materialises a SubagentRunItem card on the first subagent_updated", () => {
    const state = runEvents([
      subagentUpdated({ subagent_id: "a", status: "running", name: "Explore" }),
    ]);
    const all = cards(state);
    expect(all).toHaveLength(1);
    expect(all[0].subagents).toHaveLength(1);
    expect(all[0].subagents[0]).toMatchObject({
      id: "a",
      name: "Explore",
      status: "running",
    });
  });

  it("joins a second subagent to the same contiguous card", () => {
    const state = runEvents([
      subagentUpdated({ subagent_id: "a", status: "running", name: "A" }),
      subagentUpdated({ subagent_id: "b", status: "running", name: "B" }),
    ]);
    const all = cards(state);
    expect(all).toHaveLength(1);
    expect(all[0].subagents.map((s) => s.id)).toEqual(["a", "b"]);
  });

  it("opens a NEW card when a parent item interrupts the spawn group", () => {
    const state = runEvents([
      subagentUpdated({ subagent_id: "a", status: "running", name: "A" }),
      // A parent-flow assistant message breaks contiguity.
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: { kind: "assistant_text", text: "interlude" },
      },
      subagentUpdated({ subagent_id: "b", status: "running", name: "B" }),
    ]);
    expect(cards(state)).toHaveLength(2);
  });

  it("merges snapshots (non-null wins) and keeps status monotonic", () => {
    const state = runEvents([
      subagentUpdated({ subagent_id: "a", status: "running", name: "Impl" }),
      subagentUpdated({ subagent_id: "a", status: "running", model: "opus" }),
      subagentUpdated({
        subagent_id: "a",
        status: "completed",
        result_text: "All done",
        tool_use_count: 28,
      }),
      // A stray default-pending update must not revive a finished row.
      subagentUpdated({ subagent_id: "a", status: "pending" }),
    ]);
    const sub = cards(state)[0].subagents[0];
    expect(sub).toMatchObject({
      name: "Impl",
      model: "opus",
      status: "completed",
      resultText: "All done",
      toolUseCount: 28,
    });
  });

  it("routes subagent_id-tagged items into the subagent's sub-transcript, not the parent flow", () => {
    const state = runEvents([
      subagentUpdated({ subagent_id: "a", status: "running", name: "A" }),
      subItem("a", {
        kind: "tool_use",
        tool_name: "Read",
        tool_use_id: "x",
        input: { file_path: "/f" },
      }),
      subItem("a", { kind: "tool_result", tool_use_id: "x", content: "ok", is_error: false }),
      subItem("a", { kind: "assistant_text", text: "child says hi" }),
    ]);
    // The parent transcript holds ONLY the card (no leaked child rows).
    expect(state.messages).toHaveLength(1);
    const sub = cards(state)[0].subagents[0];
    const childTool = sub.items.find((i) => i.kind === "tool_call");
    expect(childTool).toMatchObject({ tool_name: "Read", status: "done" });
    expect(
      sub.items.some(
        (i) => i.kind === "assistant_message" && i.text === "child says hi",
      ),
    ).toBe(true);
  });

  it("streams subagent content_delta into the sub-transcript", () => {
    const state = runEvents([
      subagentUpdated({ subagent_id: "a", status: "running", name: "A" }),
      {
        type: "content_delta",
        thread_id: "t1",
        turn_id: "turn-1",
        subagent_id: "a",
        delta: { kind: "text", text: "Hel" },
      },
      {
        type: "content_delta",
        thread_id: "t1",
        turn_id: "turn-1",
        subagent_id: "a",
        delta: { kind: "text", text: "lo" },
      },
    ]);
    const sub = cards(state)[0].subagents[0];
    const msg = sub.items.find((i) => i.kind === "assistant_message");
    expect(msg).toMatchObject({ text: "Hello" });
    // The parent thread's streaming flag is owned by session_state, not
    // by child deltas.
    expect(state.streaming).toBe(false);
  });

  it("tags a bubbled request_opened with its subagent_id", () => {
    const state = runEvents([
      subagentUpdated({ subagent_id: "a", status: "running", name: "A" }),
      {
        type: "request_opened",
        thread_id: "t1",
        turn_id: "turn-1",
        request_id: "req-1",
        request_kind: "command",
        payload: {},
        tool_use_id: null,
        subagent_id: "a",
      },
    ]);
    const req = state.messages.find(
      (m): m is PermissionRequestItem => m.kind === "permission_request",
    );
    expect(req?.subagent_id).toBe("a");
    // Bubbles into the parent flow so the turn can't stall invisibly.
    expect(state.pendingRequestIds).toContain("req-1");
  });

  it("caps a subagent's retained items", () => {
    const events: ProviderRuntimeEvent[] = [
      subagentUpdated({ subagent_id: "a", status: "running", name: "A" }),
    ];
    for (let i = 0; i < 600; i++) {
      events.push(subItem("a", { kind: "assistant_text", text: `m${i}` }));
    }
    const state = runEvents(events);
    const sub = cards(state)[0].subagents[0];
    expect(sub.items.length).toBeLessThanOrEqual(500);
  });

  it("survives a persisted replay unchanged (old rows still deserialize)", () => {
    // subagent_updated without a `type`-less payload — replay path uses
    // applyEvent; here we just confirm the reducer is a pure function of
    // the same events applied twice.
    const events: ProviderRuntimeEvent[] = [
      subagentUpdated({ subagent_id: "a", status: "running", name: "A" }),
      subItem("a", { kind: "assistant_text", text: "hi" }),
    ];
    const once = runEvents(events);
    __resetReducerIdCounterForTests();
    const twice = runEvents(events);
    expect(cards(twice)[0].subagents[0].items.length).toBe(
      cards(once)[0].subagents[0].items.length,
    );
  });

  // ── Run-state settlement (issue #153) ──

  it("a parent-scoped tool_result settles a stuck running subagent (the issue shape)", () => {
    const state = runEvents([
      subagentUpdated({
        subagent_id: "s1",
        status: "running",
        name: "Explore",
        parent_item_id: "spawn-1",
      }),
      // The raw spawning tool_result flows through the PARENT flow (no
      // subagent_id) because the adapter's demux lost track — and no
      // terminal snapshot ever follows.
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: { kind: "tool_result", tool_use_id: "spawn-1", content: "ok", is_error: false },
      },
    ]);
    const sub = cards(state)[0].subagents[0];
    expect(sub.status).toBe("completed");
    expect(sub.statusAssumed).toBe(true);
  });

  it("a parent-scoped error tool_result settles the subagent as failed", () => {
    const state = runEvents([
      subagentUpdated({ subagent_id: "s1", status: "running", parent_item_id: "spawn-1" }),
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: { kind: "tool_result", tool_use_id: "spawn-1", content: "boom", is_error: true },
      },
    ]);
    expect(cards(state)[0].subagents[0].status).toBe("failed");
  });

  it("a subagent-TAGGED tool_result does NOT settle the subagent view", () => {
    const state = runEvents([
      subagentUpdated({ subagent_id: "s1", status: "running", parent_item_id: "spawn-1" }),
      subItem("s1", { kind: "tool_use", tool_name: "Bash", tool_use_id: "child-1", input: {} }),
      // Tagged with subagent_id → routes into the sub-transcript, never
      // the parent settle path.
      subItem("s1", { kind: "tool_result", tool_use_id: "child-1", content: "ok", is_error: false }),
    ]);
    const sub = cards(state)[0].subagents[0];
    expect(sub.status).toBe("running");
    expect(sub.statusAssumed).toBeUndefined();
  });

  it("session_state_changed closed interrupts running subagents even when not streaming", () => {
    const running = runEvents([
      subagentUpdated({ subagent_id: "s1", status: "running", name: "A" }),
    ]);
    expect(running.streaming).toBe(false);
    const closed = applyEvent(running, {
      type: "session_state_changed",
      thread_id: "t1",
      status: { status: "closed" },
    });
    expect(cards(closed)[0].subagents[0].status).toBe("interrupted");
    expect(cards(closed)[0].subagents[0].statusAssumed).toBe(true);
  });

  it("session_state_changed error interrupts running subagents", () => {
    const running = runEvents([
      subagentUpdated({ subagent_id: "s1", status: "running", name: "A" }),
    ]);
    const errored = applyEvent(running, {
      type: "session_state_changed",
      thread_id: "t1",
      status: { status: "error", message: "boom" },
    });
    expect(cards(errored)[0].subagents[0].status).toBe("interrupted");
  });

  it("a new user turn (not streaming) interrupts leftover running subagents", () => {
    const running = runEvents([
      subagentUpdated({ subagent_id: "s1", status: "running", name: "A" }),
    ]);
    const next = appendUserMessage(running, "another question");
    expect(cards(next)[0].subagents[0].status).toBe("interrupted");
  });

  it("a queued follow-up (streaming) leaves running subagents alone", () => {
    const running = runEvents([
      subagentUpdated({ subagent_id: "s1", status: "running", name: "A" }),
    ]);
    const streaming = { ...running, streaming: true };
    const next = appendUserMessage(streaming, "queued behind the active turn");
    expect(cards(next)[0].subagents[0].status).toBe("running");
  });
});

describe("agent-chat reducer — workflows", () => {
  beforeEach(() => {
    __resetReducerIdCounterForTests();
  });

  function workflowUpdated(workflow: Record<string, unknown>): ProviderRuntimeEvent {
    return {
      type: "workflow_updated",
      thread_id: "t1",
      workflow: workflow as never,
    } as ProviderRuntimeEvent;
  }
  function workflowSubagentUpdated(
    subagent: Record<string, unknown>,
  ): ProviderRuntimeEvent {
    return {
      type: "subagent_updated",
      thread_id: "t1",
      subagent: subagent as never,
    } as ProviderRuntimeEvent;
  }
  function workflowSubItem(
    subagentId: string,
    item: Record<string, unknown>,
  ): ProviderRuntimeEvent {
    return {
      type: "item_completed",
      thread_id: "t1",
      turn_id: "turn-1",
      subagent_id: subagentId,
      item: item as never,
    } as ProviderRuntimeEvent;
  }
  function workflows(state: ChatThreadState): WorkflowRunItem[] {
    return state.messages.filter(
      (m): m is WorkflowRunItem => m.kind === "workflow_run",
    );
  }

  const samplePhases = [
    { title: "Explore", detail: "scan for suspects" },
    { title: "Fix", detail: "apply the fixes" },
  ];

  it("materialises a WorkflowRunItem on the first workflow_updated", () => {
    const state = runEvents([
      workflowUpdated({
        workflow_id: "wf1",
        status: "running",
        name: "Bug Hunt",
        description: "Find and fix bugs",
        phases: samplePhases,
      }),
    ]);
    const all = workflows(state);
    expect(all).toHaveLength(1);
    expect(all[0]).toMatchObject({
      workflowId: "wf1",
      status: "running",
      name: "Bug Hunt",
      description: "Find and fix bugs",
    });
    expect(all[0].plannedPhases).toEqual(samplePhases);
    expect(all[0].phases.map((p) => p.title)).toEqual(["Explore", "Fix"]);
    expect(all[0].phases.every((p) => p.agents.length === 0)).toBe(true);
  });

  it("merges workflow snapshots (non-null wins) and keeps status monotonic", () => {
    const state = runEvents([
      workflowUpdated({ workflow_id: "wf1", status: "running", name: "Bug Hunt" }),
      workflowUpdated({ workflow_id: "wf1", status: "completed", result_text: "done", total_tokens: 500 }),
      // A stray duplicate "running" snapshot must not revive a finished run.
      workflowUpdated({ workflow_id: "wf1", status: "running" }),
    ]);
    const wf = workflows(state)[0];
    expect(wf.status).toBe("completed");
    expect(wf.resultText).toBe("done");
    expect(wf.totalTokens).toBe(500);
    expect(wf.name).toBe("Bug Hunt");
  });

  it("routes a workflow_id-tagged subagent_updated into the matching phase, not a subagent_run card", () => {
    const state = runEvents([
      workflowUpdated({ workflow_id: "wf1", status: "running", phases: samplePhases }),
      workflowSubagentUpdated({
        subagent_id: "sub-a",
        status: "running",
        name: "Explorer",
        workflow_id: "wf1",
        phase: "Explore",
      }),
    ]);
    // No generic subagent_run card — the agent lives inside the workflow.
    expect(state.messages.filter((m) => m.kind === "subagent_run")).toHaveLength(0);
    const wf = workflows(state)[0];
    const explorePhase = wf.phases.find((p) => p.title === "Explore")!;
    expect(explorePhase.agents).toHaveLength(1);
    expect(explorePhase.agents[0]).toMatchObject({ id: "sub-a", name: "Explorer", status: "running" });
  });

  it("falls back to the last planned phase title when the subagent has no phase hint", () => {
    const state = runEvents([
      workflowUpdated({ workflow_id: "wf1", status: "running", phases: samplePhases }),
      workflowSubagentUpdated({ subagent_id: "sub-a", status: "running", workflow_id: "wf1" }),
    ]);
    const wf = workflows(state)[0];
    const fixPhase = wf.phases.find((p) => p.title === "Fix")!;
    expect(fixPhase.agents.map((a) => a.id)).toEqual(["sub-a"]);
  });

  it("synthesizes a 'Run' phase bucket when there are no planned phases and no phase hint", () => {
    const state = runEvents([
      workflowUpdated({ workflow_id: "wf1", status: "running" }),
      workflowSubagentUpdated({ subagent_id: "sub-a", status: "running", workflow_id: "wf1" }),
    ]);
    const wf = workflows(state)[0];
    expect(wf.phases.map((p) => p.title)).toEqual(["Run"]);
    expect(wf.phases[0].agents.map((a) => a.id)).toEqual(["sub-a"]);
  });

  it("routes subagent_id-tagged items into the workflow phase's sub-transcript", () => {
    const state = runEvents([
      workflowUpdated({ workflow_id: "wf1", status: "running", phases: samplePhases }),
      workflowSubagentUpdated({
        subagent_id: "sub-a",
        status: "running",
        workflow_id: "wf1",
        phase: "Explore",
      }),
      workflowSubItem("sub-a", { kind: "assistant_text", text: "child says hi" }),
    ]);
    const wf = workflows(state)[0];
    const explorePhase = wf.phases.find((p) => p.title === "Explore")!;
    expect(
      explorePhase.agents[0].items.some(
        (i) => i.kind === "assistant_message" && i.text === "child says hi",
      ),
    ).toBe(true);
    // The parent transcript holds only the workflow item — no leaked rows.
    expect(state.messages).toHaveLength(1);
  });

  it("merges a subagent snapshot arriving before the workflow item via the generic fallback", () => {
    // Out-of-order defensive path: the update is never dropped even
    // though the workflow item hasn't been seen yet.
    const state = runEvents([
      workflowSubagentUpdated({
        subagent_id: "sub-a",
        status: "running",
        workflow_id: "wf-not-seen-yet",
      }),
    ]);
    expect(workflows(state)).toHaveLength(0);
    const cards = state.messages.filter(
      (m): m is SubagentRunItem => m.kind === "subagent_run",
    );
    expect(cards).toHaveLength(1);
    expect(cards[0].subagents[0].id).toBe("sub-a");
  });

  it("links a workflow to a pending approval request via tool_use_id and flips to pending_approval", () => {
    let state = runEvents([
      workflowUpdated({ workflow_id: "wf1", status: "running" }),
    ]);
    state = applyEvent(state, {
      type: "request_opened",
      thread_id: "t1",
      turn_id: "turn-1",
      request_id: "req-wf",
      request_kind: "other",
      payload: {},
      tool_use_id: "wf1",
    });
    const wf = workflows(state)[0];
    expect(wf.status).toBe("pending_approval");
    expect(wf.approvalRequestId).toBe("req-wf");
  });

  it("resumes to running when the gating approval is allowed", () => {
    let state = runEvents([workflowUpdated({ workflow_id: "wf1", status: "running" })]);
    state = applyEvent(state, {
      type: "request_opened",
      thread_id: "t1",
      turn_id: "turn-1",
      request_id: "req-wf",
      request_kind: "other",
      payload: {},
      tool_use_id: "wf1",
    });
    state = applyEvent(state, {
      type: "request_resolved",
      thread_id: "t1",
      request_id: "req-wf",
      decision: { decision: "allow" },
    });
    const wf = workflows(state)[0];
    expect(wf.status).toBe("running");
    // Stays linked after resolution so transcript-slots keeps suppressing
    // the standalone resolved permission block (no stray "Allowed" row).
    expect(wf.approvalRequestId).toBe("req-wf");
  });

  it("stops the workflow when the gating approval is denied", () => {
    let state = runEvents([workflowUpdated({ workflow_id: "wf1", status: "running" })]);
    state = applyEvent(state, {
      type: "request_opened",
      thread_id: "t1",
      turn_id: "turn-1",
      request_id: "req-wf",
      request_kind: "other",
      payload: {},
      tool_use_id: "wf1",
    });
    state = applyEvent(state, {
      type: "request_resolved",
      thread_id: "t1",
      request_id: "req-wf",
      decision: { decision: "deny", message: "no" },
    });
    const wf = workflows(state)[0];
    expect(wf.status).toBe("stopped");
  });

  it("stops a still-running workflow when its turn ends in error", () => {
    let state = runEvents([workflowUpdated({ workflow_id: "wf1", status: "running" })]);
    state = applyEvent(state, {
      type: "turn_completed",
      thread_id: "t1",
      turn_id: "turn-1",
      status: { kind: "error", subtype: "interrupted", message: "session interrupted" },
      usage: null,
    });
    const wf = workflows(state)[0];
    expect(wf.status).toBe("stopped");
  });

  it("leaves a completed workflow's status alone when the turn later errors", () => {
    let state = runEvents([
      workflowUpdated({ workflow_id: "wf1", status: "running" }),
      workflowUpdated({ workflow_id: "wf1", status: "completed" }),
    ]);
    state = applyEvent(state, {
      type: "turn_completed",
      thread_id: "t1",
      turn_id: "turn-1",
      status: { kind: "error", subtype: "interrupted", message: "session interrupted" },
      usage: null,
    });
    const wf = workflows(state)[0];
    expect(wf.status).toBe("completed");
  });

  it("a parent-scoped tool_result for the Workflow tool settles the run + its agents (issue #153)", () => {
    const state = runEvents([
      workflowUpdated({ workflow_id: "wf1", status: "running", phases: samplePhases }),
      workflowSubagentUpdated({
        subagent_id: "sub-a",
        status: "running",
        workflow_id: "wf1",
        phase: "Explore",
      }),
      // The Workflow tool's own tool_result (tool_use_id == workflow_id)
      // leaks through the parent flow with no terminal workflow snapshot.
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: { kind: "tool_result", tool_use_id: "wf1", content: "done", is_error: false },
      },
    ]);
    const wf = workflows(state)[0];
    expect(wf.status).toBe("completed");
    const agent = wf.phases.find((p) => p.title === "Explore")!.agents[0];
    expect(agent.status).toBe("completed");
    expect(agent.statusAssumed).toBe(true);
  });

  it("an error tool_result for the Workflow tool fails the run and interrupts its agents", () => {
    const state = runEvents([
      workflowUpdated({ workflow_id: "wf1", status: "running", phases: samplePhases }),
      workflowSubagentUpdated({
        subagent_id: "sub-a",
        status: "running",
        workflow_id: "wf1",
        phase: "Explore",
      }),
      {
        type: "item_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        item: { kind: "tool_result", tool_use_id: "wf1", content: "boom", is_error: true },
      },
    ]);
    const wf = workflows(state)[0];
    expect(wf.status).toBe("failed");
    const agent = wf.phases.find((p) => p.title === "Explore")!.agents[0];
    expect(agent.status).toBe("interrupted");
  });
});

describe("dead-run detection (issue #154)", () => {
  beforeEach(() => {
    __resetReducerIdCounterForTests();
  });

  const running: ProviderRuntimeEvent = {
    type: "session_state_changed",
    thread_id: "t1",
    status: { status: "running", active_turn: "turn-1" },
  };
  const stalled: ProviderRuntimeEvent = {
    type: "run_stalled",
    thread_id: "t1",
    silent_for_secs: 640,
  };

  it("run_stalled sets the stalled marker", () => {
    const state = runEvents([running, stalled]);
    expect(state.stalled).toEqual({ silentForSecs: 640 });
  });

  it("the next real activity clears the stalled marker", () => {
    const afterStall = runEvents([running, stalled]);
    expect(afterStall.stalled).not.toBeNull();
    const afterDelta = applyEvent(afterStall, {
      type: "content_delta",
      thread_id: "t1",
      turn_id: "turn-1",
      delta: { kind: "text", text: "back alive" },
    });
    expect(afterDelta.stalled).toBeNull();
  });

  it("a re-emitted run_stalled does not thrash state when unchanged", () => {
    const once = runEvents([running, stalled]);
    const twice = applyEvent(once, stalled);
    expect(twice).toBe(once);
  });

  it("turn_completed with child_exited marks the thread interrupted", () => {
    const state = runEvents([
      running,
      {
        type: "turn_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        status: {
          kind: "error",
          subtype: "child_exited",
          message: "sidecar exited unexpectedly",
        },
        usage: null,
      },
    ]);
    expect(state.interrupted).toBe(true);
    expect(state.streaming).toBe(false);
  });

  it("a user-initiated stop (interrupted subtype) does NOT set interrupted", () => {
    const state = runEvents([
      running,
      {
        type: "turn_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        status: {
          kind: "error",
          subtype: "interrupted",
          message: "session interrupted",
        },
        usage: null,
      },
    ]);
    expect(state.interrupted).toBe(false);
  });

  it("session_state_changed error while streaming sets interrupted (belt-and-braces)", () => {
    const state = runEvents([
      running,
      {
        type: "session_state_changed",
        thread_id: "t1",
        status: { status: "error", message: "server unreachable" },
      },
    ]);
    expect(state.interrupted).toBe(true);
  });

  it("a clean ready/closed stop does not set interrupted", () => {
    const ready = runEvents([
      running,
      {
        type: "session_state_changed",
        thread_id: "t1",
        status: { status: "ready" },
      },
    ]);
    expect(ready.interrupted).toBe(false);
  });

  it("a fresh running turn clears the interrupted flag", () => {
    const interrupted = runEvents([
      running,
      {
        type: "turn_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        status: {
          kind: "error",
          subtype: "child_exited",
          message: "dead",
        },
        usage: null,
      },
    ]);
    expect(interrupted.interrupted).toBe(true);
    const resumed = applyEvent(interrupted, {
      type: "session_state_changed",
      thread_id: "t1",
      status: { status: "running", active_turn: "turn-2" },
    });
    expect(resumed.interrupted).toBe(false);
  });

  it("appending a user message clears the interrupted flag", () => {
    const interrupted = runEvents([
      running,
      {
        type: "turn_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        status: { kind: "error", subtype: "child_exited", message: "dead" },
        usage: null,
      },
    ]);
    const resumed = appendUserMessage(interrupted, "Continue");
    expect(resumed.interrupted).toBe(false);
  });

  it("a success completion after a child_exited completion clears interrupted (settle-race)", () => {
    // The stdout-reader / exit-watchdog race can persist BOTH a synthetic
    // child_exited completion and the real success completion for the same
    // turn. Replaying them in that order must end up NOT interrupted so a
    // genuinely-finished run is never mislabelled after a restart.
    const state = runEvents([
      running,
      {
        type: "turn_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        status: { kind: "error", subtype: "child_exited", message: "dead" },
        usage: null,
      },
      {
        type: "turn_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        status: { kind: "success" },
        usage: null,
      },
    ]);
    expect(state.interrupted).toBe(false);
  });

  it("rolling back a failed resume send restores the interrupted flag", () => {
    // A Continue send optimistically clears `interrupted`; if the send RPC
    // then fails, the rollback must re-arm it (passing the pre-append value)
    // so the "Run interrupted" divider + Continue chip survive one failed
    // click instead of vanishing with no recovery affordance.
    const interrupted = runEvents([
      running,
      {
        type: "turn_completed",
        thread_id: "t1",
        turn_id: "turn-1",
        status: { kind: "error", subtype: "child_exited", message: "dead" },
        usage: null,
      },
    ]);
    expect(interrupted.interrupted).toBe(true);
    const optimistic = appendUserMessage(interrupted, "Continue", undefined, "nonce-1");
    expect(optimistic.interrupted).toBe(false);
    const rolledBack = removeUserMessageByNonce(optimistic, "nonce-1", true);
    expect(rolledBack.interrupted).toBe(true);
    // The optimistic bubble is gone (no orphan left behind).
    expect(
      rolledBack.messages.some(
        (m) => m.kind === "user_message" && m.text === "Continue",
      ),
    ).toBe(false);
  });

  it("rolling back a normal (non-resume) send does NOT arm interrupted", () => {
    const clean = appendUserMessage(
      createEmptyThreadState(),
      "hello",
      undefined,
      "nonce-2",
    );
    const rolledBack = removeUserMessageByNonce(clean, "nonce-2", false);
    expect(rolledBack.interrupted).toBe(false);
  });
});
