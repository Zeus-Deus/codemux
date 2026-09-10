// Tests for `src/methods/get-session-messages.ts` and its registry entry.
//
// The SDK's transcript read is swapped out through the
// `setSessionMessageReaderForTests` seam — the same DI pattern
// `list-sessions` uses — so nothing here touches the real filesystem or
// the user's actual conversation history.

import { afterEach, describe, expect, test } from "bun:test";
import type { SessionMessage } from "@anthropic-ai/claude-agent-sdk";

import {
  buildMethods,
  InvalidParamsError,
  type MethodHandler,
} from "../src/methods/index.ts";
import type { EventEmitter } from "../src/session.ts";
import {
  getSessionMessages,
  MAX_PAGE_SIZE,
  resetSessionMessageReaderForTests,
  setSessionMessageReaderForTests,
  sliceHistory,
  type GetSessionMessagesResult,
} from "../src/methods/get-session-messages.ts";

const noopEmitter: EventEmitter = {
  notification() {},
};

function registryMethod(name: string): MethodHandler {
  const handler = buildMethods(noopEmitter)[name];
  if (!handler) throw new Error(`no registered method named ${name}`);
  return handler;
}

const SESSION_ID = "11111111-2222-3333-4444-555555555555";

/** A transcript of `count` alternating user/assistant turns. The `uuid`
 *  encodes the chronological index so slices can be checked by eye. */
function transcript(count: number): SessionMessage[] {
  return Array.from({ length: count }, (_, i) => ({
    type: i % 2 === 0 ? "user" : "assistant",
    uuid: `m${i}`,
    session_id: SESSION_ID,
    message: {
      role: i % 2 === 0 ? "user" : "assistant",
      content: `turn ${i}`,
    },
    parent_tool_use_id: null,
  }));
}

type ReaderCall = {
  sessionId: string;
  options: { dir?: string; includeSystemMessages: boolean };
};

/** Install a canned transcript and record what the method asked for. */
function stubTranscript(rows: SessionMessage[]): { calls: ReaderCall[] } {
  const calls: ReaderCall[] = [];
  setSessionMessageReaderForTests(async (sessionId, options) => {
    calls.push({ sessionId, options });
    return rows;
  });
  return { calls };
}

function uuids(result: GetSessionMessagesResult): string[] {
  return result.messages.map((m) => m.uuid);
}

afterEach(() => {
  resetSessionMessageReaderForTests();
});

// ---------------------------------------------------------------------------
// Param validation
// ---------------------------------------------------------------------------

describe("get-session-messages param validation", () => {
  test("rejects a non-object payload", async () => {
    stubTranscript([]);
    const method = registryMethod("get-session-messages");
    for (const bad of [undefined, null, "history", 7, ["a"]]) {
      await expect(method(bad)).rejects.toBeInstanceOf(InvalidParamsError);
    }
  });

  test("sessionId is required and must not be blank", async () => {
    const { calls } = stubTranscript([]);
    const method = registryMethod("get-session-messages");
    await expect(method({ limit: 10 })).rejects.toThrow(
      "sessionId must be a string",
    );
    await expect(method({ sessionId: 42, limit: 10 })).rejects.toThrow(
      "sessionId must be a string",
    );
    await expect(method({ sessionId: "   ", limit: 10 })).rejects.toThrow(
      "sessionId must not be empty",
    );
    expect(calls).toEqual([]);
  });

  test("limit is required and must be an integer of at least 1", async () => {
    const { calls } = stubTranscript([]);
    const method = registryMethod("get-session-messages");
    for (const limit of [undefined, 0, -3, 2.5]) {
      await expect(
        method({ sessionId: SESSION_ID, limit }),
      ).rejects.toThrow("limit must be an integer >= 1");
    }
    await expect(
      method({ sessionId: SESSION_ID, limit: "10" }),
    ).rejects.toThrow("limit must be a number when present");
    expect(calls).toEqual([]);
  });

  test("beforeOffset must be a non-negative integer when present", async () => {
    const { calls } = stubTranscript([]);
    const method = registryMethod("get-session-messages");
    for (const beforeOffset of [-1, 1.5]) {
      await expect(
        method({ sessionId: SESSION_ID, limit: 10, beforeOffset }),
      ).rejects.toThrow("beforeOffset must be an integer >= 0 when present");
    }
    await expect(
      method({ sessionId: SESSION_ID, limit: 10, beforeOffset: "0" }),
    ).rejects.toThrow("beforeOffset must be a number when present");
    expect(calls).toEqual([]);
  });

  test("dir must be a string when present", async () => {
    stubTranscript([]);
    await expect(
      registryMethod("get-session-messages")({
        sessionId: SESSION_ID,
        limit: 10,
        dir: 12,
      }),
    ).rejects.toThrow("dir must be a string when present");
  });

  test("every rejection above is an InvalidParamsError", async () => {
    stubTranscript([]);
    const method = registryMethod("get-session-messages");
    for (const bad of [
      { limit: 10 },
      { sessionId: SESSION_ID, limit: 0 },
      { sessionId: SESSION_ID, limit: 10, beforeOffset: -1 },
    ]) {
      await expect(method(bad)).rejects.toBeInstanceOf(InvalidParamsError);
    }
  });
});

// ---------------------------------------------------------------------------
// Paging arithmetic (pure)
// ---------------------------------------------------------------------------

describe("sliceHistory", () => {
  const all = ["a", "b", "c", "d", "e", "f", "g"];

  test("no beforeOffset returns the newest page and its start index", () => {
    expect(sliceHistory(all, { limit: 3 })).toEqual({
      messages: ["e", "f", "g"],
      total: 7,
      offset: 4,
    });
  });

  test("a page larger than the history returns everything from 0", () => {
    expect(sliceHistory(all, { limit: 50 })).toEqual({
      messages: all,
      total: 7,
      offset: 0,
    });
  });

  test("beforeOffset returns the page immediately before that index", () => {
    expect(sliceHistory(all, { limit: 2, beforeOffset: 4 })).toEqual({
      messages: ["c", "d"],
      total: 7,
      offset: 2,
    });
  });

  test("walking back from the newest page reaches offset 0 exactly", () => {
    const first = sliceHistory(all, { limit: 3 });
    const second = sliceHistory(all, { limit: 3, beforeOffset: first.offset });
    const third = sliceHistory(all, { limit: 3, beforeOffset: second.offset });
    expect(second).toEqual({ messages: ["b", "c", "d"], total: 7, offset: 1 });
    expect(third).toEqual({ messages: ["a"], total: 7, offset: 0 });
    expect(sliceHistory(all, { limit: 3, beforeOffset: 0 })).toEqual({
      messages: [],
      total: 7,
      offset: 0,
    });
  });

  test("a beforeOffset smaller than the page clamps at the beginning", () => {
    expect(sliceHistory(all, { limit: 5, beforeOffset: 2 })).toEqual({
      messages: ["a", "b"],
      total: 7,
      offset: 0,
    });
  });

  test("a beforeOffset past the end behaves like the newest page", () => {
    expect(sliceHistory(all, { limit: 2, beforeOffset: 99 })).toEqual({
      messages: ["f", "g"],
      total: 7,
      offset: 5,
    });
  });

  test("an empty history is an empty page at offset 0", () => {
    expect(sliceHistory([], { limit: 10 })).toEqual({
      messages: [],
      total: 0,
      offset: 0,
    });
  });

  test("the page size is clamped to MAX_PAGE_SIZE", () => {
    const big = Array.from({ length: 1200 }, (_, i) => i);
    const page = sliceHistory(big, { limit: 10_000 });
    expect(MAX_PAGE_SIZE).toBe(500);
    expect(page.messages.length).toBe(500);
    expect(page.offset).toBe(700);
    expect(page.messages[0]).toBe(700);
    expect(page.messages[499]).toBe(1199);
  });
});

// ---------------------------------------------------------------------------
// Method behaviour through the registry
// ---------------------------------------------------------------------------

describe("get-session-messages", () => {
  test("omitting beforeOffset returns the last N messages", async () => {
    stubTranscript(transcript(10));
    const result = (await registryMethod("get-session-messages")({
      sessionId: SESSION_ID,
      limit: 4,
    })) as GetSessionMessagesResult;
    expect(uuids(result)).toEqual(["m6", "m7", "m8", "m9"]);
    expect(result).toMatchObject({ total: 10, offset: 6 });
  });

  test("beforeOffset pages backwards and clamps at 0", async () => {
    stubTranscript(transcript(10));
    const method = registryMethod("get-session-messages");
    const middle = (await method({
      sessionId: SESSION_ID,
      limit: 4,
      beforeOffset: 6,
    })) as GetSessionMessagesResult;
    expect(uuids(middle)).toEqual(["m2", "m3", "m4", "m5"]);
    expect(middle).toMatchObject({ total: 10, offset: 2 });

    const head = (await method({
      sessionId: SESSION_ID,
      limit: 4,
      beforeOffset: middle.offset,
    })) as GetSessionMessagesResult;
    expect(uuids(head)).toEqual(["m0", "m1"]);
    expect(head).toMatchObject({ total: 10, offset: 0 });
  });

  test("records pass through untouched", async () => {
    const rows = transcript(3);
    const rich: SessionMessage = {
      type: "assistant",
      uuid: "rich",
      session_id: SESSION_ID,
      parent_tool_use_id: null,
      message: {
        role: "assistant",
        model: "example-model",
        content: [
          { type: "thinking", thinking: "..." },
          { type: "text", text: "Running it." },
          {
            type: "tool_use",
            id: "toolu_1",
            name: "Bash",
            input: { command: "ls" },
          },
        ],
        usage: { input_tokens: 1, output_tokens: 2 },
      },
    };
    // The SDK adds fields (e.g. `timestamp`) its type does not declare;
    // they must survive the round trip too.
    const withExtra = { ...rich, timestamp: "2026-08-20T10:30:00.000Z" };
    stubTranscript([...rows, withExtra as SessionMessage]);
    const result = await getSessionMessages({ sessionId: SESSION_ID, limit: 1 });
    expect(result.messages).toEqual([withExtra as SessionMessage]);
    expect(result.messages[0]).toBe(withExtra as SessionMessage);
  });

  test("limit is clamped to 500 before slicing", async () => {
    stubTranscript(transcript(600));
    const result = (await registryMethod("get-session-messages")({
      sessionId: SESSION_ID,
      limit: 5000,
    })) as GetSessionMessagesResult;
    expect(result.messages.length).toBe(500);
    expect(result).toMatchObject({ total: 600, offset: 100 });
    expect(result.messages[0]?.uuid).toBe("m100");
  });

  test("the SDK is asked once per call, without system messages", async () => {
    const { calls } = stubTranscript(transcript(3));
    await registryMethod("get-session-messages")({
      sessionId: SESSION_ID,
      limit: 2,
      beforeOffset: 2,
    });
    expect(calls).toEqual([
      { sessionId: SESSION_ID, options: { includeSystemMessages: false } },
    ]);
    expect("dir" in (calls[0]?.options as object)).toBe(false);
  });

  test("dir is passed through to the SDK when given", async () => {
    const { calls } = stubTranscript(transcript(3));
    await registryMethod("get-session-messages")({
      sessionId: SESSION_ID,
      limit: 2,
      dir: "/home/dev/projects/app",
    });
    expect(calls[0]?.options).toEqual({
      dir: "/home/dev/projects/app",
      includeSystemMessages: false,
    });
  });

  test("a whitespace-padded sessionId is trimmed before reaching the SDK", async () => {
    const { calls } = stubTranscript([]);
    await registryMethod("get-session-messages")({
      sessionId: `  ${SESSION_ID}  `,
      limit: 2,
    });
    expect(calls[0]?.sessionId).toBe(SESSION_ID);
  });

  test("an empty transcript is an empty page, not an error", async () => {
    stubTranscript([]);
    const result = await getSessionMessages({ sessionId: SESSION_ID, limit: 10 });
    expect(result).toEqual({ messages: [], total: 0, offset: 0 });
  });

  test("an SDK rejection propagates with its own message", async () => {
    setSessionMessageReaderForTests(async () => {
      throw new Error(`Session ${SESSION_ID} not found`);
    });
    const method = registryMethod("get-session-messages");
    const attempt = method({ sessionId: SESSION_ID, limit: 10 });
    await expect(attempt).rejects.toThrow(`Session ${SESSION_ID} not found`);
    await expect(attempt).rejects.not.toBeInstanceOf(InvalidParamsError);
  });
});
