import { describe, expect, it } from "vitest";

import type { ProviderRuntimeEvent } from "@/tauri/events";

import { replayPayloads } from "./hydrate";

function user(text: string): string {
  return JSON.stringify({ type: "user_message", thread_id: "t", text });
}

function event(e: ProviderRuntimeEvent): string {
  return JSON.stringify(e);
}

describe("replayPayloads", () => {
  it("returns an empty quiescent state for [] input", () => {
    const state = replayPayloads([]);
    expect(state.messages).toEqual([]);
    expect(state.streaming).toBe(false);
    expect(state.pendingRequestIds).toEqual([]);
    expect(state.nextSeq).toBe(0);
  });

  it("appends user messages from the synthetic envelope", () => {
    const state = replayPayloads([user("hello"), user("world")]);
    expect(state.messages).toHaveLength(2);
    expect(state.messages[0]).toMatchObject({
      kind: "user_message",
      text: "hello",
      seq: 0,
    });
    expect(state.messages[1]).toMatchObject({
      kind: "user_message",
      text: "world",
      seq: 1,
    });
  });

  it("maps a user_message envelope's images onto the item", () => {
    const state = replayPayloads([
      JSON.stringify({
        type: "user_message",
        thread_id: "t",
        text: "see attached",
        images: [
          { path: "/tmp/codemux/a.png", media_type: "image/png" },
          { path: "/tmp/codemux/b.jpg" },
        ],
      }),
    ]);
    expect(state.messages).toHaveLength(1);
    const item = state.messages[0];
    expect(item.kind).toBe("user_message");
    if (item.kind === "user_message") {
      // `path` → `src`; `media_type` → `mediaType` (undefined when the
      // backend omitted it).
      expect(item.images).toEqual([
        { src: "/tmp/codemux/a.png", mediaType: "image/png" },
        { src: "/tmp/codemux/b.jpg", mediaType: undefined },
      ]);
    }
  });

  it("drops malformed image entries and keeps text-only when images is absent", () => {
    const state = replayPayloads([
      JSON.stringify({
        type: "user_message",
        thread_id: "t",
        text: "mixed",
        images: [
          { path: "/ok.png", media_type: "image/png" },
          { media_type: "image/png" }, // no path → dropped
          "not-an-object", // → dropped
          null, // → dropped
        ],
      }),
      user("plain"),
    ]);
    const [withImages, plain] = state.messages;
    if (withImages.kind === "user_message") {
      expect(withImages.images).toEqual([
        { src: "/ok.png", mediaType: "image/png" },
      ]);
    }
    // A turn with no `images` field never gains one.
    expect("images" in plain).toBe(false);
  });

  it("rebuilds an assistant message from a completed item", () => {
    const state = replayPayloads([
      user("hi"),
      event({
        type: "item_completed",
        thread_id: "t",
        turn_id: "turn-1",
        item: { kind: "assistant_text", text: "hello back" },
      }),
    ]);
    expect(state.messages).toHaveLength(2);
    expect(state.messages[1]).toMatchObject({
      kind: "assistant_message",
      text: "hello back",
      streaming: false,
    });
  });

  it("rebuilds a tool call + result pairing across separate events", () => {
    const state = replayPayloads([
      user("read x"),
      event({
        type: "item_completed",
        thread_id: "t",
        turn_id: "turn-1",
        item: {
          kind: "tool_use",
          tool_name: "Read",
          input: { path: "/x" },
          tool_use_id: "tu-1",
        },
      }),
      event({
        type: "item_completed",
        thread_id: "t",
        turn_id: "turn-1",
        item: {
          kind: "tool_result",
          tool_use_id: "tu-1",
          content: "file contents",
          is_error: false,
        },
      }),
    ]);
    const toolCall = state.messages.find((m) => m.kind === "tool_call");
    expect(toolCall).toBeDefined();
    expect(toolCall).toMatchObject({
      kind: "tool_call",
      tool_name: "Read",
      tool_use_id: "tu-1",
      status: "done",
      result_content: "file contents",
    });
  });

  it("links a permission_request to its tool_call by tool_use_id", () => {
    const state = replayPayloads([
      event({
        type: "item_completed",
        thread_id: "t",
        turn_id: "turn-1",
        item: {
          kind: "tool_use",
          tool_name: "Bash",
          input: { command: "ls" },
          tool_use_id: "tu-1",
        },
      }),
      event({
        type: "request_opened",
        thread_id: "t",
        turn_id: "turn-1",
        request_id: "req-1",
        request_kind: "tool",
        payload: {},
        tool_use_id: "tu-1",
      }),
      event({
        type: "request_resolved",
        thread_id: "t",
        request_id: "req-1",
        decision: { decision: "allow_for_session" },
      }),
    ]);
    const toolCall = state.messages.find((m) => m.kind === "tool_call");
    const permReq = state.messages.find(
      (m) => m.kind === "permission_request",
    );
    expect(toolCall).toBeDefined();
    expect(permReq).toBeDefined();
    if (toolCall?.kind === "tool_call") {
      expect(toolCall.approval_request_id).toBe("req-1");
    }
    if (permReq?.kind === "permission_request") {
      expect(permReq.resolution).toEqual({
        state: "resolved",
        decision: { decision: "allow_for_session" },
      });
    }
    // Resolved requests are removed from pendingRequestIds. The
    // post-replay invariant — pendingRequestIds is empty — also
    // belt-and-braces clears it inside replayPayloads.
    expect(state.pendingRequestIds).toEqual([]);
  });

  it("clears streaming and pendingRequestIds even if the source ended mid-turn", () => {
    // A truncated history (process killed mid-stream) ends up with a
    // streaming assistant_message and an unresolved request. Replay
    // must not leave the resumed slice in those states or the UI
    // would render a frozen spinner / pending-approval card forever.
    const state = replayPayloads([
      event({
        type: "item_completed",
        thread_id: "t",
        turn_id: "turn-1",
        item: { kind: "assistant_text", text: "thinking…" },
      }),
      event({
        type: "request_opened",
        thread_id: "t",
        turn_id: "turn-1",
        request_id: "req-stuck",
        request_kind: "tool",
        payload: {},
        tool_use_id: null,
      }),
      // No turn_completed. No request_resolved.
    ]);
    expect(state.streaming).toBe(false);
    expect(state.pendingRequestIds).toEqual([]);
  });

  it("skips malformed JSON without affecting the rest of the replay", () => {
    const state = replayPayloads([
      user("first"),
      "not-valid-json",
      "{not even close}",
      user("second"),
    ]);
    expect(state.messages.map((m) => m.kind)).toEqual([
      "user_message",
      "user_message",
    ]);
    if (state.messages[0].kind === "user_message") {
      expect(state.messages[0].text).toBe("first");
    }
    if (state.messages[1].kind === "user_message") {
      expect(state.messages[1].text).toBe("second");
    }
  });

  it("skips envelopes without a `type` field", () => {
    const state = replayPayloads([
      JSON.stringify({ thread_id: "t", text: "no type" }),
      user("real"),
    ]);
    expect(state.messages).toHaveLength(1);
  });

  it("skips a user_message envelope missing its text field", () => {
    const state = replayPayloads([
      JSON.stringify({ type: "user_message", thread_id: "t" }),
      user("real"),
    ]);
    expect(state.messages).toHaveLength(1);
  });

  it("skips JSON null and primitives", () => {
    const state = replayPayloads(["null", "42", '"a string"', user("ok")]);
    expect(state.messages).toHaveLength(1);
  });

  it("preserves seq monotonicity across mixed envelope types", () => {
    // Order is the property that lets the rendered transcript match
    // the original conversation. Verify that `nextSeq` advances past
    // every appended item.
    const state = replayPayloads([
      user("a"),
      event({
        type: "item_completed",
        thread_id: "t",
        turn_id: "turn-1",
        item: { kind: "assistant_text", text: "b" },
      }),
      user("c"),
    ]);
    const seqs = state.messages.map((m) => m.seq);
    // Strictly increasing.
    for (let i = 1; i < seqs.length; i++) {
      expect(seqs[i]).toBeGreaterThan(seqs[i - 1]);
    }
    expect(state.nextSeq).toBeGreaterThan(seqs[seqs.length - 1]);
  });

  it("handles a long replay without losing rows or reordering", () => {
    const payloads: string[] = [];
    for (let i = 0; i < 100; i++) {
      payloads.push(user(`u-${i}`));
      payloads.push(
        event({
          type: "item_completed",
          thread_id: "t",
          turn_id: `turn-${i}`,
          item: { kind: "assistant_text", text: `a-${i}` },
        }),
      );
      payloads.push(
        event({
          type: "turn_completed",
          thread_id: "t",
          turn_id: `turn-${i}`,
          status: { kind: "success" },
          usage: null,
        }),
      );
    }
    const state = replayPayloads(payloads);
    // 100 user + 100 assistant = 200 rendered items (turn_completed
    // success is silent in the reducer).
    expect(state.messages).toHaveLength(200);
    for (let i = 0; i < 100; i++) {
      const userItem = state.messages[i * 2];
      const asstItem = state.messages[i * 2 + 1];
      expect(userItem.kind).toBe("user_message");
      expect(asstItem.kind).toBe("assistant_message");
      if (userItem.kind === "user_message") {
        expect(userItem.text).toBe(`u-${i}`);
      }
      if (asstItem.kind === "assistant_message") {
        expect(asstItem.text).toBe(`a-${i}`);
      }
    }
  });

  it("rebuilds a sealed reasoning block from thinking deltas + completion", () => {
    const state = replayPayloads([
      user("why?"),
      event({
        type: "content_delta",
        thread_id: "t",
        turn_id: "turn-1",
        delta: { kind: "thinking", text: "Because " },
      }),
      event({
        type: "content_delta",
        thread_id: "t",
        turn_id: "turn-1",
        delta: { kind: "thinking", text: "of the cache." },
      }),
      event({
        type: "item_completed",
        thread_id: "t",
        turn_id: "turn-1",
        item: { kind: "assistant_thinking", text: "Because of the cache." },
      }),
      event({
        type: "item_completed",
        thread_id: "t",
        turn_id: "turn-1",
        item: { kind: "assistant_text", text: "It's the cache." },
      }),
    ]);
    const reasoning = state.messages.find((m) => m.kind === "reasoning");
    expect(reasoning).toBeDefined();
    if (reasoning?.kind === "reasoning") {
      expect(reasoning.text).toBe("Because of the cache.");
      expect(reasoning.streaming).toBe(false);
    }
    // Reasoning precedes the assistant prose in the rebuilt transcript.
    expect(state.messages.map((m) => m.kind)).toEqual([
      "user_message",
      "reasoning",
      "assistant_message",
    ]);
  });

  it("seals a reasoning block left streaming by a transcript truncated mid-thinking", () => {
    // History cut off after thinking deltas but before the completion or
    // any boundary item. The belt-and-braces pass must seal the block so a
    // restored thread never renders a perpetual "Thinking…".
    const state = replayPayloads([
      event({
        type: "content_delta",
        thread_id: "t",
        turn_id: "turn-1",
        delta: { kind: "thinking", text: "half a thought" },
      }),
      // No completion. No boundary. No turn_completed.
    ]);
    const reasoning = state.messages.find((m) => m.kind === "reasoning");
    expect(reasoning).toBeDefined();
    if (reasoning?.kind === "reasoning") {
      expect(reasoning.text).toBe("half a thought");
      expect(reasoning.streaming).toBe(false);
    }
    expect(state.streaming).toBe(false);
  });

  it("emits a turn_ended marker for an error-status turn_completed", () => {
    // Errors are surfaced inline so the resumed transcript matches
    // what the user saw originally — silently dropping them would
    // lose context for the next turn.
    const state = replayPayloads([
      event({
        type: "item_completed",
        thread_id: "t",
        turn_id: "turn-err",
        item: { kind: "assistant_text", text: "oops" },
      }),
      event({
        type: "turn_completed",
        thread_id: "t",
        turn_id: "turn-err",
        status: { kind: "error", subtype: "rate_limit", message: "429" },
        usage: null,
      }),
    ]);
    const ended = state.messages.find((m) => m.kind === "turn_ended");
    expect(ended).toBeDefined();
    if (ended?.kind === "turn_ended") {
      expect(ended.status.kind).toBe("error");
    }
  });
});
