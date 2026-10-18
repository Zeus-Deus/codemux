// Tests for steering a running turn (`steerTurn` / the `steer-turn` RPC).

import { afterEach, beforeEach, expect, test } from "bun:test";

import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";

import {
  ClaudeSession,
  resetInterruptExitTimeoutForTests,
  resetQueryFactoryForTests,
  setInterruptExitTimeoutForTests,
  setQueryFactoryForTests,
  type EventEmitter,
  type SessionStartInput,
} from "../src/session.ts";
import { buildMethods, _resetSessionsForTests } from "../src/methods/index.ts";
import { asQuery, FakeQuery } from "./fake-query.ts";

const emit: EventEmitter = { notification() {} };

function input(): SessionStartInput {
  return {
    threadId: "t-steer",
    cwd: "/tmp/test-cwd",
    pathToClaudeCodeExecutable: "/usr/bin/claude",
  };
}

let fakes: FakeQuery[] = [];

beforeEach(() => {
  fakes = [];
  setQueryFactoryForTests((args) => {
    const fake = new FakeQuery(args);
    fakes.push(fake);
    return asQuery(fake);
  });
  setInterruptExitTimeoutForTests(50);
});

afterEach(() => {
  resetQueryFactoryForTests();
  resetInterruptExitTimeoutForTests();
  _resetSessionsForTests();
});

const settle = () => new Promise((r) => setTimeout(r, 10));

test("steerTurn pushes next-priority guidance carrying the host uuid", async () => {
  const session = new ClaudeSession(input(), emit);
  await session.sendTurn({ text: "first" });
  await session.steerTurn({
    text: "also do this",
    uuid: "steer-1",
    images: [{ mediaType: "image/png", dataBase64: "AAA" }],
  });
  await settle();
  const fake = fakes[0] as FakeQuery;
  expect(fake.capturedPrompts).toHaveLength(2);
  const steer = fake.capturedPrompts[1];
  expect(steer?.priority).toBe("next");
  expect(steer?.uuid as string).toBe("steer-1");
  expect(steer?.parent_tool_use_id).toBeNull();
  const content = steer?.message.content as Array<{ type: string; text?: string }>;
  expect(content.map((block) => block.type)).toEqual(["image", "text"]);
  expect(content[1]?.text).toBe("also do this");
  // Ordinary turns are not prioritised, but carry a uuid so the CLI echoes
  // them, which the host waits for before steering.
  expect(fake.capturedPrompts[0]?.priority).toBeUndefined();
  expect(typeof fake.capturedPrompts[0]?.uuid).toBe("string");
  await session.close();
});

test("steerTurn is rejected once an interrupt killed the query", async () => {
  const session = new ClaudeSession(input(), emit);
  const fake = fakes[0] as FakeQuery;
  fake.interruptError = Object.assign(new Error("aborted"), { name: "AbortError" });
  await session.interrupt();
  await expect(session.steerTurn({ text: "late", uuid: "steer-2" })).rejects.toThrow(
    "no running turn to steer",
  );
  // The dead query's prompt stream never sees the rejected guidance, and no
  // replacement query is built for it.
  expect(fakes).toHaveLength(1);
  expect(fake.capturedPrompts).toHaveLength(0);
  await session.close();
});

test("steerTurn is rejected while an interrupt is still ending the turn", async () => {
  const session = new ClaudeSession(input(), emit);
  const fake = fakes[0] as FakeQuery;
  let release!: () => void;
  fake.interrupt = () => new Promise<void>((resolve) => (release = resolve));
  const stopping = session.interrupt();
  await expect(session.steerTurn({ text: "late", uuid: "steer-3" })).rejects.toThrow(
    "no running turn to steer",
  );
  release();
  await stopping;
  await session.close();
});

test("steer-turn RPC validates params and forwards to the session", async () => {
  const methods = buildMethods(emit);
  await methods["start-session"]?.({
    threadId: "t-steer",
    cwd: "/tmp/test-cwd",
    pathToClaudeCodeExecutable: "/usr/bin/claude",
  });
  await methods["send-turn"]?.({ threadId: "t-steer", text: "first" });
  await expect(methods["steer-turn"]?.({ threadId: "t-steer", text: "x" })).rejects.toThrow();
  const result = await methods["steer-turn"]?.({
    threadId: "t-steer",
    text: "guidance",
    uuid: "steer-4",
  });
  expect(result).toEqual({ steered: true });
  await settle();
  const prompt = (fakes[0] as FakeQuery).capturedPrompts[1];
  expect(prompt?.uuid as string).toBe("steer-4");
  expect(prompt?.message.content).toBe("guidance");
});

function events(): { emit: EventEmitter; methods: string[] } {
  const methods: string[] = [];
  return { emit: { notification: (method) => methods.push(method) }, methods };
}

test("Stop with unread guidance ends the CLI process instead of a soft interrupt", async () => {
  // A soft interrupt leaves queued prompts in the CLI, which runs the
  // guidance as its next turn: the newer Stop would not win.
  const { emit: recording, methods } = events();
  const session = new ClaudeSession(input(), recording);
  await session.sendTurn({ text: "first" });
  await session.steerTurn({ text: "guidance", uuid: "steer-5" });
  const fake = fakes[0] as FakeQuery;
  await session.interrupt();
  expect(fake.interruptCalls).toBe(0);
  expect(fake.closed).toBe(true);
  expect(methods).toContain("turn-interrupted");
  expect(methods).not.toContain("session-ended");
  expect(methods).not.toContain("session-error");
  // The next turn resumes on a fresh query.
  await session.sendTurn({ text: "next" });
  await settle();
  expect(fakes).toHaveLength(2);
  expect(fakes[1]?.capturedPrompts[0]?.message.content).toBe("next");
  await session.close();
});

test("Stop after the guidance was consumed interrupts softly", async () => {
  const session = new ClaudeSession(input(), emit);
  await session.sendTurn({ text: "first" });
  await session.steerTurn({ text: "guidance", uuid: "steer-6" });
  const fake = fakes[0] as FakeQuery;
  fake.emit({
    type: "result",
    subtype: "success",
    parent_tool_use_id: null,
    user_message_uuids: ["first", "steer-6"],
  } as unknown as SDKMessage);
  await settle();
  await session.interrupt();
  expect(fake.interruptCalls).toBe(1);
  expect(fake.closed).toBe(false);
  await session.close();
});

test("guidance arriving after a soft Stop is rejected until the next turn", async () => {
  // The CLI runs whatever is queued once the stopped turn aborts, so late
  // guidance would restart work the user just stopped.
  const session = new ClaudeSession(input(), emit);
  await session.sendTurn({ text: "first" });
  await session.interrupt();
  await expect(session.steerTurn({ text: "late", uuid: "steer-7" })).rejects.toThrow(
    "no running turn to steer",
  );
  await session.sendTurn({ text: "next" });
  await session.steerTurn({ text: "fresh", uuid: "steer-8" });
  await settle();
  const uuids = (fakes[0] as FakeQuery).capturedPrompts.map((p) => p.uuid as string | undefined);
  expect(uuids).toContain("steer-8");
  expect(uuids).not.toContain("steer-7");
  await session.close();
});

test("each query gets an abort controller so a Stop can end its CLI at once", async () => {
  const session = new ClaudeSession(input(), emit);
  await session.sendTurn({ text: "first" });
  await session.steerTurn({ text: "guidance", uuid: "steer-9" });
  const signal = (fakes[0] as FakeQuery).capturedOptions?.abortController?.signal;
  expect(signal?.aborted).toBe(false);
  await session.interrupt();
  expect(signal?.aborted).toBe(true);
  await session.close();
});

test("a retired query that exits late stays silent once its replacement runs", async () => {
  const methods: string[] = [];
  const messages: unknown[] = [];
  const session = new ClaudeSession(input(), {
    notification(method, params) {
      methods.push(method);
      if (method === "sdk-message") messages.push((params as { message: unknown }).message);
    },
  });
  await session.sendTurn({ text: "first" });
  await session.steerTurn({ text: "guidance", uuid: "steer-10" });
  const old = fakes[0] as FakeQuery;
  // The old CLI outlives the retirement timeout.
  old.close = () => {
    old.closed = true;
  };
  await session.interrupt();
  expect(methods).toContain("turn-interrupted");
  await session.sendTurn({ text: "next" });
  expect(fakes).toHaveLength(2);

  old.emit({ type: "assistant", parent_tool_use_id: null } as unknown as SDKMessage);
  old.errorStream(new Error("SIGTERM"));
  await settle();
  expect(messages).toHaveLength(0);
  expect(methods).not.toContain("session-ended");
  expect(methods).not.toContain("session-error");

  // The replacement is still the live query: a steer reaches it and a new
  // turn does not rebuild it.
  await session.steerTurn({ text: "fresh", uuid: "steer-11" });
  await session.sendTurn({ text: "again" });
  await settle();
  expect(fakes).toHaveLength(2);
  expect(fakes[1]?.capturedPrompts.map((p) => p.uuid as string)).toContain("steer-11");
  await session.close();
});
