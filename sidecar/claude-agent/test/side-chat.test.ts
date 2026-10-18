// Tests for `/btw` side chats: the fork options, read-only enforcement and
// the notification stream, driven through the RPC handlers with FakeQuery.

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";

import type { EventEmitter } from "../src/session.ts";
import { resetQueryFactoryForTests, setQueryFactoryForTests } from "../src/session.ts";
import {
  buildSideChatOptions,
  SIDE_CHAT_DISALLOWED_TOOLS,
  sideChatCanUseTool,
  sideChatPrompt,
} from "../src/side-chat.ts";
import { buildMethods, _resetSessionsForTests } from "../src/methods/index.ts";
import { asQuery, FakeQuery } from "./fake-query.ts";

const START = {
  sideId: "side-1",
  cwd: "/tmp/repo",
  resume: "main-sdk-session",
  pathToClaudeCodeExecutable: "/usr/bin/claude",
  text: "what is the plan?",
  appendSystemPrompt: "side chat",
};

function harness() {
  const events: Array<{ method: string; params: Record<string, unknown> }> = [];
  const emit: EventEmitter = {
    notification(method, params) {
      events.push({ method, params: params as Record<string, unknown> });
    },
  };
  let fake: FakeQuery | null = null;
  setQueryFactoryForTests((args) => {
    fake = new FakeQuery(args);
    return asQuery(fake);
  });
  return { methods: buildMethods(emit), events, fake: () => fake! };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 5));

const delta = (text: string) =>
  ({
    type: "stream_event",
    parent_tool_use_id: null,
    event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
  }) as unknown as SDKMessage;

const result = (text: string) =>
  ({ type: "result", subtype: "success", result: text, session_id: "fork" }) as unknown as SDKMessage;

beforeEach(() => _resetSessionsForTests());
afterEach(() => {
  resetQueryFactoryForTests();
  _resetSessionsForTests();
});

describe("side chat options", () => {
  test("fork the main session without persisting a transcript", () => {
    const options = buildSideChatOptions(START);
    expect(options.resume).toBe("main-sdk-session");
    expect(options.forkSession).toBe(true);
    expect(options.persistSession).toBe(false);
    expect(options.cwd).toBe("/tmp/repo");
  });

  test("offer only read tools and disable hooks, settings and MCP", () => {
    const options = buildSideChatOptions(START);
    expect(options.tools).toEqual(["Read", "Grep", "Glob"]);
    for (const tool of ["Bash", "Edit", "Write", "NotebookEdit", "Agent", "Task"]) {
      expect(options.disallowedTools).toContain(tool);
    }
    expect(options.settingSources).toEqual([]);
    expect((options.settings as Record<string, unknown>).disableAllHooks).toBe(true);
    expect(options.mcpServers).toEqual({});
    expect(options.strictMcpConfig).toBe(true);
    expect(options.permissionMode).toBe("default");
  });

  test("carry the main chat's model and effort", () => {
    const options = buildSideChatOptions({ ...START, model: "claude-opus-5-5", effort: "high" });
    expect(options.model).toBe("claude-opus-5-5");
    expect(options.effort as unknown).toBe("high");
    const bare = buildSideChatOptions(START);
    expect(bare.model).toBeUndefined();
    expect(bare.effort).toBeUndefined();
  });

  test("normalize ultracode and ultrathink like a main session", () => {
    const ultracode = buildSideChatOptions({ ...START, effort: "ultracode" });
    expect(ultracode.effort as unknown).toBe("xhigh");
    expect((ultracode.settings as Record<string, unknown>).ultracode).toBe(true);
    expect((ultracode.settings as Record<string, unknown>).disableAllHooks).toBe(true);
    const ultrathink = buildSideChatOptions({ ...START, effort: "ultrathink" });
    expect(ultrathink.effort).toBeUndefined();
    expect((ultrathink.settings as Record<string, unknown>).ultracode).toBeUndefined();
    expect(sideChatPrompt("why?", "ultrathink")).toBe("Ultrathink:\nwhy?");
    expect(sideChatPrompt("Ultrathink:\nwhy?", "ultrathink")).toBe("Ultrathink:\nwhy?");
    expect(sideChatPrompt("why?", "high")).toBe("why?");
  });

  test("canUseTool denies anything that would act", async () => {
    const signal = new AbortController().signal;
    const ctx = { signal, suggestions: [] } as never;
    for (const tool of [...SIDE_CHAT_DISALLOWED_TOOLS, "mcp__x__y"]) {
      expect((await sideChatCanUseTool(tool, {}, ctx)).behavior).toBe("deny");
    }
    expect((await sideChatCanUseTool("Read", { file_path: "a" }, ctx)).behavior).toBe("allow");
  });
});

describe("side chat RPC", () => {
  test("streams the answer, accepts follow-ups on the same fork, and stops", async () => {
    const { methods, events, fake } = harness();
    await methods["start-side-chat"]!(START);
    await flush();
    expect(fake().capturedOptions?.forkSession).toBe(true);
    expect(fake().capturedPrompts.map((m) => m.message.content)).toEqual(["what is the plan?"]);

    fake().emit(delta("Ship "));
    fake().emit(delta("it."));
    fake().emit(result("Ship it."));
    await flush();
    expect(events.map((e) => e.method)).toEqual([
      "side-chat-delta",
      "side-chat-delta",
      "side-chat-turn-completed",
    ]);
    expect(events.every((e) => e.params.sideId === "side-1")).toBe(true);
    expect(events[2]!.params.text).toBe("Ship it.");

    await methods["side-chat-send"]!({ text: "and then?" });
    await flush();
    expect(fake().capturedPrompts.map((m) => m.message.content)).toEqual([
      "what is the plan?",
      "and then?",
    ]);

    await methods["stop-side-chat"]!({});
    expect(fake().closed).toBe(true);
    expect(events.at(-1)!.method).toBe("side-chat-ended");
    expect(await methods["stop-side-chat"]!({})).toEqual({ alreadyClosed: true });
  });

  test("start-side-chat applies the effort to the query and every question", async () => {
    const { methods, fake } = harness();
    await methods["start-side-chat"]!({ ...START, model: "claude-opus-5-5", effort: "ultrathink" });
    await flush();
    expect(fake().capturedOptions?.model).toBe("claude-opus-5-5");
    expect(fake().capturedOptions?.effort).toBeUndefined();
    await methods["side-chat-send"]!({ text: "and then?" });
    await flush();
    expect(fake().capturedPrompts.map((m) => m.message.content)).toEqual([
      "Ultrathink:\nwhat is the plan?",
      "Ultrathink:\nand then?",
    ]);
    await methods["stop-side-chat"]!({});

    const second = harness();
    await second.methods["start-side-chat"]!({ ...START, effort: "max" });
    await flush();
    expect(second.fake().capturedOptions?.effort as unknown).toBe("max");
    expect(second.fake().capturedPrompts.map((m) => m.message.content)).toEqual(["what is the plan?"]);
    await second.methods["stop-side-chat"]!({});
  });

  test("subagent and non-text deltas are not forwarded", async () => {
    const { methods, events, fake } = harness();
    await methods["start-side-chat"]!(START);
    fake().emit({ ...(delta("inner") as object), parent_tool_use_id: "tool-1" } as unknown as SDKMessage);
    fake().emit({
      type: "stream_event",
      parent_tool_use_id: null,
      event: { type: "content_block_delta", delta: { type: "thinking_delta", thinking: "hmm" } },
    } as unknown as SDKMessage);
    await flush();
    expect(events).toEqual([]);
    await methods["stop-side-chat"]!({});
  });

  test("an error result surfaces its message", async () => {
    const { methods, events, fake } = harness();
    await methods["start-side-chat"]!(START);
    fake().emit({
      type: "result",
      subtype: "error_during_execution",
      errors: ["No conversation found with session ID: main-sdk-session"],
    } as unknown as SDKMessage);
    await flush();
    expect(events[0]!.method).toBe("side-chat-error");
    expect(String(events[0]!.params.text)).toContain("No conversation found");
    await methods["stop-side-chat"]!({});
  });

  test("refuses a second side chat, empty input and sends without a chat", async () => {
    const { methods } = harness();
    await expect(methods["side-chat-send"]!({ text: "x" })).rejects.toThrow("no side chat");
    await expect(methods["start-side-chat"]!({ ...START, text: "  " })).rejects.toThrow("empty");
    await expect(methods["start-side-chat"]!({ ...START, resume: "" })).rejects.toThrow("resume");
    await methods["start-side-chat"]!(START);
    await expect(methods["start-side-chat"]!(START)).rejects.toThrow("already running");
    await expect(methods["side-chat-send"]!({ text: " " })).rejects.toThrow("empty");
    await methods["stop-side-chat"]!({});
  });
});
