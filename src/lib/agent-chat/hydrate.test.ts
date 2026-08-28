import { describe, expect, it } from "vitest";

import type { ProviderRuntimeEvent } from "@/tauri/events";

import {
  applyReplayTail,
  lastTurnUnsettled,
  parseReplayPayloads,
  replayPayloads,
  replayTimed,
} from "./hydrate";
import { applyEvent, createEmptyThreadState } from "./reducer";
import { findSubagentView, runningSubagentEntries } from "./subagents";
import type {
  ChatThreadState,
  PermissionRequestItem,
  WorkflowRunItem,
} from "./types";

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

  it("stamps searchable prose with its durable persisted event id", () => {
    const state = replayTimed([
      {
        event: {
          type: "user_message",
          thread_id: "t",
          text: "find this prompt",
        },
        persistedId: 17,
      },
      {
        event: {
          type: "item_completed",
          thread_id: "t",
          turn_id: "turn-search",
          item: { kind: "assistant_text", text: "find this answer" },
        },
        persistedId: 18,
      },
    ]);

    expect(state.messages[0]).toMatchObject({
      kind: "user_message",
      source_event_id: 17,
    });
    expect(state.messages[1]).toMatchObject({
      kind: "assistant_message",
      turn_id: "turn-search",
      source_event_id: 18,
    });
  });

  it("restores durable row timestamps for stable worked-time labels", () => {
    const state = replayTimed([
      {
        event: { type: "user_message", thread_id: "t", text: "time this" },
        createdAtMs: 1_000,
      },
      {
        event: {
          type: "item_completed",
          thread_id: "t",
          turn_id: "turn-1",
          item: { kind: "assistant_text", text: "done" },
        },
        createdAtMs: 2_000,
      },
      {
        event: {
          type: "turn_completed",
          thread_id: "t",
          turn_id: "turn-1",
          status: { kind: "success" },
          usage: null,
        },
        createdAtMs: 6_500,
      },
    ]);

    expect(state.messages[0]).toMatchObject({
      kind: "user_message",
      created_at: 1_000,
    });
    expect(state.messages[state.messages.length - 1]).toMatchObject({
      kind: "turn_ended",
      completed_at: 6_500,
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

  it("expires the rendered request, not only its pending id, when the source ended mid-turn", () => {
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
    const request = state.messages.find(
      (m): m is PermissionRequestItem => m.kind === "permission_request",
    );
    expect(request?.resolution).toMatchObject({
      state: "failed",
      reason: "stale_provider_callback",
    });
  });

  it("preserves an open request when the backend confirms its turn is still live", () => {
    const state = replayPayloads(
      [
        event({
          type: "request_opened",
          thread_id: "t",
          turn_id: "turn-live",
          request_id: "req-live",
          request_kind: "user-input",
          payload: { questions: [] },
          tool_use_id: null,
        }),
      ],
      { runLive: true },
    );
    const request = state.messages.find(
      (m): m is PermissionRequestItem => m.kind === "permission_request",
    );
    expect(request?.resolution.state).toBe("pending");
    expect(state.pendingRequestIds).toEqual(["req-live"]);
  });

  describe("provider-aware orphan expiry", () => {
    function openRequest(): string[] {
      return [
        user("run the thing"),
        event({
          type: "request_opened",
          thread_id: "t",
          turn_id: "turn-1",
          request_id: "req-orphan",
          request_kind: "tool",
          payload: {},
          tool_use_id: null,
        }),
      ];
    }

    function soleRequest(
      state: ReturnType<typeof replayPayloads>,
    ): PermissionRequestItem | undefined {
      return state.messages.find(
        (m): m is PermissionRequestItem => m.kind === "permission_request",
      );
    }

    it("keeps an OpenCode request actionable with no live turn", () => {
      // OpenCode's permissions live in its own HTTP server, and the
      // backend's `pending_requests_survive_session_restart` opt-in lets
      // `agent_chat_respond_to_request` re-adopt that session to answer
      // them. Expiring the control here would strand a request the user
      // can still resolve — and `request_opened`'s early-return on a known
      // id means no re-broadcast could resurrect it.
      const state = replayPayloads(openRequest(), { provider: "opencode" });
      expect(soleRequest(state)?.resolution.state).toBe("pending");
      expect(state.pendingRequestIds).toEqual(["req-orphan"]);
    });

    it.each(["claude", "codex"] as const)(
      "expires a %s request with no live turn (process-local callback)",
      (provider) => {
        const state = replayPayloads(openRequest(), { provider });
        expect(soleRequest(state)?.resolution).toMatchObject({
          state: "failed",
          reason: "stale_provider_callback",
        });
        expect(state.pendingRequestIds).toEqual([]);
      },
    );

    it("expires when the provider is unknown (matches the backend default)", () => {
      const state = replayPayloads(openRequest());
      expect(soleRequest(state)?.resolution.state).toBe("failed");
      expect(state.pendingRequestIds).toEqual([]);
    });

    it("preserves a pending_approval OpenCode workflow with no live turn", () => {
      const state = replayPayloads(
        [
          user("/workflow audit"),
          event({
            type: "workflow_updated",
            thread_id: "t",
            workflow: {
              workflow_id: "wf",
              status: "pending_approval",
              name: "Audit",
              phases: [{ title: "Run", detail: null }],
            },
          } as ProviderRuntimeEvent),
          event({
            type: "request_opened",
            thread_id: "t",
            turn_id: "turn-1",
            request_id: "req-wf",
            request_kind: "other",
            payload: {},
            tool_use_id: "wf",
          }),
        ],
        { provider: "opencode" },
      );
      const wf = state.messages.find(
        (m): m is WorkflowRunItem => m.kind === "workflow_run",
      );
      expect(wf?.status).toBe("pending_approval");
      expect(soleRequest(state)?.resolution.state).toBe("pending");
    });
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
    // Each turn retains a visually silent completion marker so presentation
    // can derive stable settled-turn folds without discarding raw history.
    expect(state.messages).toHaveLength(300);
    for (let i = 0; i < 100; i++) {
      const userItem = state.messages[i * 3];
      const asstItem = state.messages[i * 3 + 1];
      const endedItem = state.messages[i * 3 + 2];
      expect(userItem.kind).toBe("user_message");
      expect(asstItem.kind).toBe("assistant_message");
      expect(endedItem.kind).toBe("turn_ended");
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

  // ── Run-state reconciliation (issue #153) ──

  it("interrupts a subagent left running by a transcript that ends mid-run", () => {
    const state = replayPayloads([
      user("delegate"),
      event({
        type: "subagent_updated",
        thread_id: "t",
        subagent: {
          subagent_id: "s1",
          status: "running",
          name: "Explore",
          parent_item_id: "tool-1",
        },
      } as ProviderRuntimeEvent),
      // No terminal snapshot, no tool_result — the stuck-forever shape.
    ]);
    const sub = findSubagentView(state.messages, "s1");
    expect(sub?.status).toBe("interrupted");
    expect(sub?.statusAssumed).toBe(true);
    // Never resurrects a live spinner in the docked bar.
    expect(runningSubagentEntries(state.messages)).toHaveLength(0);
  });

  it("self-heals: a running snapshot then a parent tool_result hydrates as completed (not interrupted)", () => {
    const state = replayPayloads([
      user("delegate"),
      event({
        type: "subagent_updated",
        thread_id: "t",
        subagent: {
          subagent_id: "s1",
          status: "running",
          name: "Explore",
          parent_item_id: "tool-1",
        },
      } as ProviderRuntimeEvent),
      // The raw parent-scoped tool_result for the spawning tool leaks
      // through (demux lost track) — the reducer derives the settlement,
      // which the hydrate reconciliation then leaves untouched.
      event({
        type: "item_completed",
        thread_id: "t",
        turn_id: "turn-1",
        item: { kind: "tool_result", tool_use_id: "tool-1", content: "ok", is_error: false },
      }),
    ]);
    const sub = findSubagentView(state.messages, "s1");
    expect(sub?.status).toBe("completed");
    expect(sub?.statusAssumed).toBe(true);
    expect(runningSubagentEntries(state.messages)).toHaveLength(0);
  });

  it("stops a workflow left running by a truncated transcript", () => {
    const state = replayPayloads([
      user("/workflow audit"),
      event({
        type: "workflow_updated",
        thread_id: "t",
        workflow: {
          workflow_id: "wf",
          status: "running",
          name: "Audit",
          phases: [{ title: "Run", detail: null }],
        },
      } as ProviderRuntimeEvent),
    ]);
    const wf = state.messages.find(
      (m): m is WorkflowRunItem => m.kind === "workflow_run",
    );
    expect(wf?.status).toBe("stopped");
  });

  it("preserves a pending_approval workflow only while its provider turn is live", () => {
    const state = replayPayloads(
      [
        user("/workflow audit"),
        event({
          type: "workflow_updated",
          thread_id: "t",
          workflow: {
            workflow_id: "wf",
            status: "pending_approval",
            name: "Audit",
            phases: [{ title: "Run", detail: null }],
          },
        } as ProviderRuntimeEvent),
        event({
          type: "request_opened",
          thread_id: "t",
          turn_id: "turn-1",
          request_id: "req-1",
          request_kind: "other",
          payload: {},
          tool_use_id: "wf",
        }),
      ],
      { runLive: true },
    );
    const wf = state.messages.find(
      (m): m is WorkflowRunItem => m.kind === "workflow_run",
    );
    expect(wf?.status).toBe("pending_approval");
  });
});

describe("lastTurnUnsettled (issue #154)", () => {
  const completed = (turnId: string): string =>
    event({
      type: "turn_completed",
      thread_id: "t",
      turn_id: turnId,
      status: { kind: "success" },
      usage: null,
    });

  /** The helper takes parsed rows now — the hydrate path parses once and
   *  shares the array with the fold. */
  const unsettled = (payloads: string[], previous = false): boolean =>
    lastTurnUnsettled(parseReplayPayloads(payloads), previous);

  it("is false for an empty thread", () => {
    expect(unsettled([])).toBe(false);
  });

  it("is true when the last user turn has no later turn_completed", () => {
    expect(unsettled([completed("turn-0"), user("still running…")])).toBe(true);
  });

  it("is false when the last user turn settled", () => {
    expect(unsettled([user("hi"), completed("turn-1")])).toBe(false);
  });

  it("is true for a settled turn followed by a fresh unsettled one", () => {
    expect(
      unsettled([user("one"), completed("turn-1"), user("two — never finished")]),
    ).toBe(true);
  });

  it("ignores malformed rows", () => {
    expect(unsettled(["not json", user("hi")])).toBe(true);
  });

  describe("cursor tail (a prefix already in memory)", () => {
    it("carries the prefix answer through a tail with no turn markers", () => {
      // A tail of pure tool/assistant items decides nothing on its own.
      expect(unsettled([], true)).toBe(true);
      expect(unsettled([], false)).toBe(false);
    });

    it("lets the tail override the prefix in both directions", () => {
      expect(unsettled([completed("turn-1")], true)).toBe(false);
      expect(unsettled([user("new turn")], false)).toBe(true);
    });

    it("matches a scan of the whole concatenated history", () => {
      const prefix = [user("one"), completed("turn-1")];
      const tail = [user("two"), completed("turn-2"), user("three")];
      expect(unsettled(tail, unsettled(prefix))).toBe(
        unsettled([...prefix, ...tail]),
      );
    });
  });
});

describe("replayPayloads interrupted/stalled (issue #154)", () => {
  it("carries interrupted from an unsettled tail and stalled stays null", () => {
    const state = replayPayloads([
      user("first"),
      event({
        type: "turn_completed",
        thread_id: "t",
        turn_id: "turn-1",
        status: { kind: "success" },
        usage: null,
      }),
      user("second — interrupted"),
    ]);
    expect(state.interrupted).toBe(true);
    expect(state.stalled).toBeNull();
  });

  it("marks interrupted when replay observed a child_exited terminal turn", () => {
    const state = replayPayloads([
      user("go"),
      event({
        type: "turn_completed",
        thread_id: "t",
        turn_id: "turn-1",
        status: {
          kind: "error",
          subtype: "child_exited",
          message: "sidecar exited unexpectedly",
        },
        usage: null,
      }),
    ]);
    expect(state.interrupted).toBe(true);
  });

  it("is not interrupted for a cleanly settled thread", () => {
    const state = replayPayloads([
      user("go"),
      event({
        type: "turn_completed",
        thread_id: "t",
        turn_id: "turn-1",
        status: { kind: "success" },
        usage: null,
      }),
    ]);
    expect(state.interrupted).toBe(false);
    expect(state.stalled).toBeNull();
  });
});

describe("replayPayloads runLive (workspace-switch remount of a live run)", () => {
  it("suppresses interrupted and streams for an unsettled tail when runLive is true", () => {
    const payloads = [
      user("first"),
      event({
        type: "turn_completed",
        thread_id: "t",
        turn_id: "turn-1",
        status: { kind: "success" },
        usage: null,
      }),
      // Unsettled tail — a live run whose terminal event isn't persisted
      // yet. Without runLive this would flag interrupted.
      user("second — still in flight"),
    ];
    expect(replayPayloads(payloads).interrupted).toBe(true);
    const live = replayPayloads(payloads, { runLive: true });
    expect(live.interrupted).toBe(false);
    expect(live.streaming).toBe(true);
  });

  it("keeps current behavior when runLive is false or the opts are absent", () => {
    const payloads = [user("only — never finished")];
    const absent = replayPayloads(payloads);
    const explicit = replayPayloads(payloads, { runLive: false });
    for (const state of [absent, explicit]) {
      expect(state.interrupted).toBe(true);
      expect(state.streaming).toBe(false);
    }
  });

  it("overrides a replay-observed child_exited when runLive is true", () => {
    const payloads = [
      user("go"),
      event({
        type: "turn_completed",
        thread_id: "t",
        turn_id: "turn-1",
        status: {
          kind: "error",
          subtype: "child_exited",
          message: "sidecar exited unexpectedly",
        },
        usage: null,
      }),
    ];
    // Default: the child_exited terminal turn flags interrupted.
    expect(replayPayloads(payloads).interrupted).toBe(true);
    // runLive means the run recovered / is alive — suppress it.
    const live = replayPayloads(payloads, { runLive: true });
    expect(live.interrupted).toBe(false);
    expect(live.streaming).toBe(true);
  });
});

describe("interim yield — a remount while delegated agents keep working", () => {
  // The regression this suite exists for: Claude Code emits an SDK `result`
  // (persisted as `turn_completed`) every time the model yields to wait on a
  // `Task` it spawned, then resumes the SAME session when the subagent
  // reports back. The reducer models that as an `interim` boundary and keeps
  // the turn live. Hydrate has to agree, or switching tabs mid-delegation
  // paints a perfectly healthy run as interrupted.
  const subagent = (status: string, activity?: string): ProviderRuntimeEvent =>
    ({
      type: "subagent_updated",
      thread_id: "t",
      subagent: {
        subagent_id: "sub-1",
        name: "Explore",
        status,
        ...(activity ? { activity } : {}),
      },
    }) as ProviderRuntimeEvent;

  const resultEvent: ProviderRuntimeEvent = {
    type: "turn_completed",
    thread_id: "t",
    turn_id: "turn-1",
    status: { kind: "success" },
  } as ProviderRuntimeEvent;

  /** A warm slice parked in an interim hold, exactly as the live stream
   *  leaves it just before the pane unmounts. */
  function interimHoldSlice(): ChatThreadState {
    let state = createEmptyThreadState();
    state = applyEvent(state, {
      type: "user_message",
      thread_id: "t",
      text: "go",
    } as ProviderRuntimeEvent);
    state = applyEvent(state, subagent("running"));
    state = applyEvent(state, resultEvent);
    return state;
  }

  it("parks in an interim hold: streaming, settled, not interrupted", () => {
    const slice = interimHoldSlice();
    expect(slice.streaming).toBe(true);
    expect(slice.interrupted).toBe(false);
    // The turn IS settled — a real `turn_completed` row was persisted.
    // This is precisely what `streaming` stopped being a proxy for.
    expect(slice.turnUnsettled).toBe(false);
    const trailing = slice.messages[slice.messages.length - 1];
    expect(trailing.kind).toBe("turn_ended");
    expect(trailing.kind === "turn_ended" && trailing.interim).toBe(true);
  });

  it("does NOT flip to interrupted when the tail is delegated-agent output", () => {
    const slice = interimHoldSlice();
    // Tab switch. The backend keeps persisting subagent rows; on remount the
    // warm tail is those rows, and `agent_chat_turn_active` now reports the
    // run live because delegated work is holding it open.
    const merged = applyReplayTail(
      slice,
      parseReplayPayloads([event(subagent("running", "reading files"))]),
      {
        runLive: true,
        previousUnsettled: slice.turnUnsettled,
        provider: "claude",
      },
    );

    expect(merged.interrupted).toBe(false);
    expect(merged.streaming).toBe(true);
    // No Continue chip, no Run-interrupted divider.
    expect(merged.interrupted && !merged.streaming).toBe(false);
    // ...and the live subagent card was NOT settled behind the run's back.
    expect(runningSubagentEntries(merged.messages)).toHaveLength(1);
  });

  it("seeds the tail scan from turnUnsettled, not from streaming", () => {
    const slice = interimHoldSlice();
    // The old seed. Kept as an explicit regression assertion: were the store
    // to go back to `slice.interrupted || slice.streaming`, this is the value
    // it would pass, and the scan would wrongly report an unsettled tail.
    const oldSeed = slice.interrupted || slice.streaming;
    expect(oldSeed).toBe(true);
    expect(slice.turnUnsettled).toBe(false);

    const tail = parseReplayPayloads([event(subagent("running", "still going"))]);
    expect(lastTurnUnsettled(tail, oldSeed)).toBe(true); // the bug
    expect(lastTurnUnsettled(tail, slice.turnUnsettled)).toBe(false); // the fix
  });

  it("still reports interrupted when the run really did die mid-turn", () => {
    // Guard against over-correcting: a tail with no completion after a user
    // turn, and a probe that says dead, must STILL raise the divider.
    let slice = createEmptyThreadState();
    slice = applyEvent(slice, {
      type: "user_message",
      thread_id: "t",
      text: "go",
    } as ProviderRuntimeEvent);
    expect(slice.turnUnsettled).toBe(true);

    const merged = applyReplayTail(slice, parseReplayPayloads([]), {
      runLive: false,
      previousUnsettled: slice.turnUnsettled,
      provider: "claude",
    });
    expect(merged.interrupted).toBe(true);
    expect(merged.streaming).toBe(false);
  });

  it("keeps turnUnsettled independent of the runLive override", () => {
    // `turnUnsettled` describes the HISTORY, so a live probe must not edit
    // it — otherwise the next tail merge seeds from a liveness verdict and
    // stops matching a full-history scan.
    let slice = createEmptyThreadState();
    slice = applyEvent(slice, {
      type: "user_message",
      thread_id: "t",
      text: "go",
    } as ProviderRuntimeEvent);

    const merged = applyReplayTail(slice, parseReplayPayloads([]), {
      runLive: true,
      previousUnsettled: slice.turnUnsettled,
      provider: "claude",
    });
    expect(merged.interrupted).toBe(false); // suppressed by runLive
    expect(merged.turnUnsettled).toBe(true); // but the history is unchanged
  });
});

describe("applyReplayTail — a live run's streaming reasoning", () => {
  const thinking = (text: string): ProviderRuntimeEvent => ({
    type: "content_delta",
    thread_id: "t",
    turn_id: "turn-1",
    delta: { kind: "thinking", text },
  });

  function reasoningBlocks(state: ChatThreadState) {
    return state.messages.filter((m) => m.kind === "reasoning");
  }

  it("leaves a reasoning block streaming when the tail merges into a live run", () => {
    // Warm slice mid-thought — the block is streaming because a LIVE
    // delta put it there — and the tail merges while the backend confirms
    // the turn is still in flight. Sealing here would make the next
    // thinking delta mint a SECOND block.
    const warm = applyEvent(createEmptyThreadState(), thinking("half a "));
    expect(reasoningBlocks(warm)).toHaveLength(1);

    const merged = applyReplayTail(warm, parseReplayPayloads([]), {
      runLive: true,
      previousUnsettled: true,
    });
    const [block] = reasoningBlocks(merged);
    expect(block.kind === "reasoning" && block.streaming).toBe(true);

    // The next delta continues the SAME block instead of starting another.
    const next = applyEvent(merged, thinking("thought"));
    const blocks = reasoningBlocks(next);
    expect(blocks).toHaveLength(1);
    expect(blocks[0].kind === "reasoning" && blocks[0].text).toBe(
      "half a thought",
    );
  });

  it("still seals when the run is NOT live", () => {
    const warm = applyEvent(createEmptyThreadState(), thinking("half a thought"));
    const merged = applyReplayTail(warm, parseReplayPayloads([]), {
      previousUnsettled: true,
    });
    const [block] = reasoningBlocks(merged);
    expect(block.kind === "reasoning" && block.streaming).toBe(false);
  });
});
