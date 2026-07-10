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

test("AskUserQuestion emits a single request-opened tagged kind=user-input", async () => {
  // Stage 1 dedup: previously AskUserQuestion triggered BOTH the
  // canUseTool `request-opened` AND a `user-input-requested`
  // side-channel, producing two separate cards in the UI with
  // different request ids. The side-channel is gone; the
  // `classifyToolKind` special case tags the single canUseTool
  // emission as `user-input` so the frontend picks the structured
  // form renderer.
  const { emit, events } = recorder();
  const pending: PendingApprovals = new Map();
  const cb = makeCanUseTool("thr", emit, pending);
  const promise = cb("AskUserQuestion", { questions: [] }, mkOptions());
  await new Promise((r) => setTimeout(r, 0));
  const opened = events.filter((e) => e.method === "request-opened");
  expect(opened).toHaveLength(1);
  expect((opened[0]?.params as { kind: string }).kind).toBe("user-input");
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

test("abort on signal rejects the pending decision, emits request-resolved deny, and clears pending", async () => {
  const { emit, events } = recorder();
  const pending: PendingApprovals = new Map();
  const cb = makeCanUseTool("thr", emit, pending);
  const controller = new AbortController();
  const promise = cb(
    "Bash",
    { command: "x" },
    { signal: controller.signal, toolUseID: "tu" },
  );
  await new Promise((r) => setTimeout(r, 0));
  const requestId = [...pending.keys()][0] as string;
  controller.abort();
  const result = await promise;
  expect(result.behavior).toBe("deny");
  if (result.behavior === "deny") {
    expect(result.message).toBe("Tool request was aborted.");
  }
  expect(pending.size).toBe(0);
  // The host mirrors this to clear its own pending-approvals map;
  // without it the dispatch queue wedges after an interrupt.
  const resolved = events.find((e) => e.method === "request-resolved");
  expect(resolved).toBeDefined();
  const params = resolved?.params as {
    requestId: string;
    decision: ApprovalDecision;
  };
  expect(params.requestId).toBe(requestId);
  expect(params.decision.behavior).toBe("deny");
  if (params.decision.behavior === "deny") {
    expect(params.decision.message).toBe("Tool request was aborted.");
  }
});

// ────────────────────────────────────────────────────────────────────
// Stage 5 — `updatedPermissions` forwarding from the resolver
// decision through to the SDK's `PermissionResult`. The bridge in
// `permissions.ts` is a `decision.updatedPermissions !== undefined`
// guard plus a bare `as PermissionUpdate[]` cast — easy to break
// without these tests catching it.
// ────────────────────────────────────────────────────────────────────

test("allow with updatedPermissions surfaces the array on PermissionResult", async () => {
  const { emit, events } = recorder();
  const pending: PendingApprovals = new Map();
  const cb = makeCanUseTool("thr", emit, pending);
  const promise = cb("Bash", { command: "ls" }, mkOptions());
  await new Promise((r) => setTimeout(r, 0));
  const requestId = [...pending.keys()][0] as string;
  const ruleArray = [
    {
      type: "addRules",
      rules: [{ toolName: "Bash" }],
      behavior: "allow",
      destination: "localSettings",
    },
  ];
  pending.get(requestId)?.({
    behavior: "allow",
    updatedPermissions: ruleArray,
  } as ApprovalDecision);

  const result = await promise;
  expect(result.behavior).toBe("allow");
  if (result.behavior === "allow") {
    // The cast at permissions.ts:191-192 is unchecked — the test
    // locks that the array survives the resolver→PermissionResult
    // hop without mutation, dropping fields, or null coercion.
    expect(result.updatedPermissions).toEqual(ruleArray);
    // updatedInput falls back to the original toolInput because
    // the decision didn't carry one.
    expect(result.updatedInput).toEqual({ command: "ls" });
  }

  // The `request-resolved` notification mirrors the decision back
  // to the Rust side; it must carry the same `updatedPermissions`
  // so observers (logs, future "rule saved" UI) see what was sent.
  const resolved = events.find((e) => e.method === "request-resolved");
  expect(resolved).toBeDefined();
  const resolvedDecision = (resolved?.params as { decision: ApprovalDecision })
    .decision;
  expect(resolvedDecision.behavior).toBe("allow");
  if (resolvedDecision.behavior === "allow") {
    expect(resolvedDecision.updatedPermissions).toEqual(ruleArray);
  }
});

test("allow WITHOUT updatedPermissions leaves the field undefined on PermissionResult", async () => {
  // One-shot allow path: the SDK distinguishes "no rule update"
  // (undefined) from "explicit empty rule update" ([]). The bridge
  // must NOT inject a default empty array — that would silently
  // bypass any other rule sources for that tool.
  const { emit } = recorder();
  const pending: PendingApprovals = new Map();
  const cb = makeCanUseTool("thr", emit, pending);
  const promise = cb("Read", { path: "/x" }, mkOptions());
  await new Promise((r) => setTimeout(r, 0));
  const requestId = [...pending.keys()][0] as string;
  pending.get(requestId)?.({ behavior: "allow" });

  const result = await promise;
  if (result.behavior === "allow") {
    expect(result.updatedPermissions).toBeUndefined();
    // Specifically NOT an empty array, NOT null.
    expect(result).not.toHaveProperty("updatedPermissions");
  } else {
    throw new Error("expected allow");
  }
});

test("allow with empty updatedPermissions array (legacy AllowForSession) preserves the empty array verbatim", async () => {
  // The Rust side's `AllowForSession` variant emits
  // `updatedPermissions: []` (per `protocol.rs::decision_allow_for_session_emits_empty_updated_permissions`).
  // The bridge must forward `[]` distinct from `undefined` — the
  // SDK uses the empty-array marker to scope an in-memory session
  // rule.
  const { emit } = recorder();
  const pending: PendingApprovals = new Map();
  const cb = makeCanUseTool("thr", emit, pending);
  const promise = cb("Bash", { command: "ls" }, mkOptions());
  await new Promise((r) => setTimeout(r, 0));
  const requestId = [...pending.keys()][0] as string;
  pending.get(requestId)?.({
    behavior: "allow",
    updatedPermissions: [],
  } as ApprovalDecision);

  const result = await promise;
  if (result.behavior === "allow") {
    expect(result.updatedPermissions).toEqual([]);
  } else {
    throw new Error("expected allow");
  }
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
