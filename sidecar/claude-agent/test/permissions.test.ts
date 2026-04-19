// Tests for `src/permissions.ts`. Drive `makeCanUseTool` directly
// with a synthesized `callbackOptions` object rather than spinning up
// a full session.

import { describe, expect, test } from "bun:test";

import type { EventEmitter } from "../src/session.ts";
import {
  makeCanUseTool,
  type ApprovalDecision,
  type PendingApprovals,
} from "../src/permissions.ts";

interface RecordedNotification {
  method: string;
  params: unknown;
}

function recorder(): {
  emit: EventEmitter;
  events: RecordedNotification[];
} {
  const events: RecordedNotification[] = [];
  return {
    emit: {
      notification(method, params) {
        events.push({ method, params });
      },
    },
    events,
  };
}

function mkOptions(overrides: Partial<Parameters<ReturnType<typeof makeCanUseTool>>[2]> = {}) {
  const controller = new AbortController();
  return {
    signal: controller.signal,
    toolUseID: "tu-1",
    ...overrides,
  };
}

test("canUseTool emits request-opened and waits for respond-to-request", async () => {
  const { emit, events } = recorder();
  const pending: PendingApprovals = new Map();
  const cb = makeCanUseTool("thr", emit, pending);
  const options = mkOptions();
  const promise = cb("Bash", { command: "ls" }, options);

  // Give the callback a tick to emit before we respond.
  await new Promise((r) => setTimeout(r, 0));
  const req = events.find((e) => e.method === "request-opened");
  expect(req).toBeDefined();
  const requestId = (req?.params as { requestId: string }).requestId;
  expect(typeof requestId).toBe("string");

  // Resolve it as allow.
  const resolver = pending.get(requestId);
  expect(resolver).toBeDefined();
  resolver?.({ behavior: "allow" });

  const result = await promise;
  expect(result.behavior).toBe("allow");
});

test("allow decision uses updatedInput when provided, otherwise original", async () => {
  const { emit } = recorder();
  const pending: PendingApprovals = new Map();
  const cb = makeCanUseTool("thr", emit, pending);
  const options = mkOptions();
  const promise = cb("Edit", { path: "/x" }, options);
  await new Promise((r) => setTimeout(r, 0));
  const requestId = [...pending.keys()][0] as string;
  pending.get(requestId)?.({
    behavior: "allow",
    updatedInput: { path: "/y" },
  });
  const result = await promise;
  expect(result.behavior).toBe("allow");
  if (result.behavior === "allow") {
    expect(result.updatedInput).toEqual({ path: "/y" });
  }
});

test("allow without updatedInput uses the original toolInput", async () => {
  const { emit } = recorder();
  const pending: PendingApprovals = new Map();
  const cb = makeCanUseTool("thr", emit, pending);
  const originalInput = { path: "/x", mode: "r" };
  const promise = cb("Read", originalInput, mkOptions());
  await new Promise((r) => setTimeout(r, 0));
  const requestId = [...pending.keys()][0] as string;
  pending.get(requestId)?.({ behavior: "allow" });
  const result = await promise;
  if (result.behavior === "allow") {
    expect(result.updatedInput).toEqual(originalInput);
  } else {
    throw new Error("expected allow");
  }
});

test("deny decision returns behavior=deny with the given message", async () => {
  const { emit } = recorder();
  const pending: PendingApprovals = new Map();
  const cb = makeCanUseTool("thr", emit, pending);
  const promise = cb("Bash", { command: "rm -rf /" }, mkOptions());
  await new Promise((r) => setTimeout(r, 0));
  const requestId = [...pending.keys()][0] as string;
  pending.get(requestId)?.({
    behavior: "deny",
    message: "never",
    interrupt: true,
  });
  const result = await promise;
  expect(result.behavior).toBe("deny");
  if (result.behavior === "deny") {
    expect(result.message).toBe("never");
    expect(result.interrupt).toBe(true);
  }
});

test("AskUserQuestion tool still goes through permission flow (emits request-opened)", async () => {
  const { emit, events } = recorder();
  const pending: PendingApprovals = new Map();
  const cb = makeCanUseTool("thr", emit, pending);
  const promise = cb("AskUserQuestion", { questions: [] }, mkOptions());
  await new Promise((r) => setTimeout(r, 0));
  expect(events.some((e) => e.method === "request-opened")).toBe(true);
  const requestId = [...pending.keys()][0] as string;
  pending.get(requestId)?.({
    behavior: "allow",
    updatedInput: { questions: [], answers: [{ idx: 0 }] },
  });
  const result = await promise;
  expect(result.behavior).toBe("allow");
});

test("ExitPlanMode emits plan-proposed and returns deny with interrupt=true", async () => {
  const { emit, events } = recorder();
  const pending: PendingApprovals = new Map();
  const cb = makeCanUseTool("thr", emit, pending);
  const result = await cb(
    "ExitPlanMode",
    { plan: "first do X" },
    mkOptions(),
  );
  expect(result.behavior).toBe("deny");
  if (result.behavior === "deny") {
    expect(result.interrupt).toBe(true);
  }
  const plan = events.find((e) => e.method === "plan-proposed");
  expect(plan).toBeDefined();
  expect((plan?.params as { plan: string }).plan).toBe("first do X");
});

test("abort on signal rejects the pending decision and translates to deny", async () => {
  const { emit } = recorder();
  const pending: PendingApprovals = new Map();
  const cb = makeCanUseTool("thr", emit, pending);
  const controller = new AbortController();
  const promise = cb(
    "Bash",
    { command: "x" },
    { signal: controller.signal, toolUseID: "tu" },
  );
  await new Promise((r) => setTimeout(r, 0));
  controller.abort();
  const result = await promise;
  expect(result.behavior).toBe("deny");
  expect(pending.size).toBe(0);
});

test("classifies tool kinds coarsely for request-opened", async () => {
  const { emit, events } = recorder();
  const pending: PendingApprovals = new Map();
  const cb = makeCanUseTool("thr", emit, pending);
  const p1 = cb("Bash", { command: "ls" }, mkOptions());
  await new Promise((r) => setTimeout(r, 0));
  const id1 = [...pending.keys()][0] as string;
  expect(
    (events.find((e) => e.method === "request-opened")?.params as { kind: string })
      .kind,
  ).toBe("command");
  pending.get(id1)?.({ behavior: "allow" });
  await p1;

  events.length = 0;
  const p2 = cb("Read", { path: "/x" }, mkOptions());
  await new Promise((r) => setTimeout(r, 0));
  const id2 = [...pending.keys()][0] as string;
  expect(
    (events.find((e) => e.method === "request-opened")?.params as { kind: string })
      .kind,
  ).toBe("file-read");
  pending.get(id2)?.({ behavior: "allow" });
  await p2;

  events.length = 0;
  const p3 = cb("Edit", { path: "/x" }, mkOptions());
  await new Promise((r) => setTimeout(r, 0));
  const id3 = [...pending.keys()][0] as string;
  expect(
    (events.find((e) => e.method === "request-opened")?.params as { kind: string })
      .kind,
  ).toBe("file-change");
  pending.get(id3)?.({ behavior: "allow" });
  await p3;
});
