import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ProviderRuntimeEvent } from "@/tauri/events";

import {
  __resetReducerIdCounterForTests,
  applyEvent,
  appendUserMessage,
  createEmptyThreadState,
  markRequestResponding,
  type Clock,
} from "./reducer";
import type {
  AssistantMessageItem,
  ChatThreadState,
  PermissionRequestItem,
  ReasoningItem,
  SubagentRunItem,
  ToolCallItem,
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
});
