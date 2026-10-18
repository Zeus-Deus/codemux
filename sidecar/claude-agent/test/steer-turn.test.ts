// Tests for steering a running turn (`steerTurn` / the `steer-turn` RPC).

import { afterEach, beforeEach, expect, test } from "bun:test";

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
  // Ordinary turns stay untouched.
  expect(fake.capturedPrompts[0]?.priority).toBeUndefined();
  expect(fake.capturedPrompts[0]?.uuid).toBeUndefined();
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
  await expect(methods["steer-turn"]?.({ threadId: "t-steer", text: "x" })).rejects.toThrow();
  const result = await methods["steer-turn"]?.({
    threadId: "t-steer",
    text: "guidance",
    uuid: "steer-4",
  });
  expect(result).toEqual({ steered: true });
  await settle();
  const prompt = (fakes[0] as FakeQuery).capturedPrompts[0];
  expect(prompt?.uuid as string).toBe("steer-4");
  expect(prompt?.message.content).toBe("guidance");
});
