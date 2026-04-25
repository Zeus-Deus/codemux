// Tests for the JSON-RPC `respond-to-request` method handler in
// `src/methods/index.ts`. This handler has a hand-written branch
// that copies `updatedPermissions` from the raw RPC params onto the
// `ApprovalDecision` it hands to the session — Stage 5's "Allow
// always" feature depends on that copy being correct.
//
// Drives the handler through `buildMethods()` and injects a fake
// session via the existing `_peekSessionsForTests()` hook so the
// test doesn't need to spin up a real Claude SDK query.

import { afterEach, describe, expect, test } from "bun:test";

import type { ApprovalDecision } from "../src/permissions.ts";
import type { ClaudeSession, EventEmitter } from "../src/session.ts";
import {
  _peekSessionsForTests,
  _resetSessionsForTests,
  buildMethods,
  InvalidParamsError,
} from "../src/methods/index.ts";

const emit: EventEmitter = { notification() {} };

function makeFakeSession(): {
  session: ClaudeSession;
  recordedDecisions: Array<{ requestId: string; decision: ApprovalDecision }>;
} {
  const recordedDecisions: Array<{
    requestId: string;
    decision: ApprovalDecision;
  }> = [];
  const session = {
    async respondToRequest(requestId: string, decision: ApprovalDecision) {
      recordedDecisions.push({ requestId, decision });
    },
  } as unknown as ClaudeSession;
  return { session, recordedDecisions };
}

afterEach(() => {
  _resetSessionsForTests();
});

describe("respond-to-request handler — updatedPermissions forwarding", () => {
  test("copies updatedPermissions verbatim onto the ApprovalDecision", async () => {
    const { session, recordedDecisions } = makeFakeSession();
    _peekSessionsForTests().set("thr-1", session);

    const handler = buildMethods(emit)["respond-to-request"]!;
    const ruleArray = [
      {
        type: "addRules",
        rules: [{ toolName: "Bash" }],
        behavior: "allow",
        destination: "localSettings",
      },
    ];

    await handler({
      threadId: "thr-1",
      requestId: "req-1",
      decision: {
        behavior: "allow",
        updatedInput: { command: "ls" },
        updatedPermissions: ruleArray,
      },
    });

    expect(recordedDecisions).toHaveLength(1);
    const recorded = recordedDecisions[0]!;
    expect(recorded.requestId).toBe("req-1");
    expect(recorded.decision.behavior).toBe("allow");
    if (recorded.decision.behavior === "allow") {
      // Locks the contract that the handler does NOT mutate the
      // array (e.g. shallow-copy then drop fields). Reference-eq
      // would also be acceptable, but deep-eq is sufficient.
      expect(recorded.decision.updatedPermissions).toEqual(ruleArray);
      expect(recorded.decision.updatedInput).toEqual({ command: "ls" });
    }
  });

  test("omits updatedPermissions when the param is absent (one-shot allow)", async () => {
    const { session, recordedDecisions } = makeFakeSession();
    _peekSessionsForTests().set("thr-1", session);

    const handler = buildMethods(emit)["respond-to-request"]!;
    await handler({
      threadId: "thr-1",
      requestId: "req-1",
      decision: { behavior: "allow", updatedInput: { command: "ls" } },
    });

    const recorded = recordedDecisions[0]!.decision;
    if (recorded.behavior === "allow") {
      // Locks the `if (decisionRaw["updatedPermissions"] !== undefined)`
      // branch — the handler must NOT inject `undefined` either,
      // since some downstream code distinguishes "field present and
      // undefined" from "field absent" (TypeScript narrowing
      // notwithstanding).
      expect("updatedPermissions" in recorded).toBe(false);
    } else {
      throw new Error("expected allow");
    }
  });

  test("preserves an empty updatedPermissions array (legacy AllowForSession)", async () => {
    const { session, recordedDecisions } = makeFakeSession();
    _peekSessionsForTests().set("thr-1", session);

    const handler = buildMethods(emit)["respond-to-request"]!;
    await handler({
      threadId: "thr-1",
      requestId: "req-1",
      decision: {
        behavior: "allow",
        updatedInput: undefined,
        updatedPermissions: [],
      },
    });

    const recorded = recordedDecisions[0]!.decision;
    if (recorded.behavior === "allow") {
      // The empty-array form is meaningful — it scopes a session-only
      // rule. Must not get coerced to `undefined`.
      expect(recorded.updatedPermissions).toEqual([]);
    } else {
      throw new Error("expected allow");
    }
  });

  test("rejects an unknown thread id with a clear error (does not silently drop the decision)", async () => {
    const handler = buildMethods(emit)["respond-to-request"]!;
    // No session registered under "thr-missing".
    await expect(
      handler({
        threadId: "thr-missing",
        requestId: "req-1",
        decision: { behavior: "allow", updatedInput: undefined },
      }),
    ).rejects.toThrow(/thr-missing/);
  });

  test("rejects unknown decision.behavior with InvalidParamsError", async () => {
    const { session } = makeFakeSession();
    _peekSessionsForTests().set("thr-1", session);

    const handler = buildMethods(emit)["respond-to-request"]!;
    await expect(
      handler({
        threadId: "thr-1",
        requestId: "req-1",
        decision: { behavior: "maybe" },
      }),
    ).rejects.toThrow(InvalidParamsError);
  });

  test("deny decision passes message + interrupt through (regression guard against the allow branch leaking)", async () => {
    const { session, recordedDecisions } = makeFakeSession();
    _peekSessionsForTests().set("thr-1", session);

    const handler = buildMethods(emit)["respond-to-request"]!;
    await handler({
      threadId: "thr-1",
      requestId: "req-1",
      decision: {
        behavior: "deny",
        message: "no thanks",
        interrupt: true,
      },
    });

    const recorded = recordedDecisions[0]!.decision;
    expect(recorded.behavior).toBe("deny");
    if (recorded.behavior === "deny") {
      expect(recorded.message).toBe("no thanks");
      expect(recorded.interrupt).toBe(true);
      // No allow-branch fields leaked.
      expect("updatedPermissions" in recorded).toBe(false);
      expect("updatedInput" in recorded).toBe(false);
    }
  });
});
