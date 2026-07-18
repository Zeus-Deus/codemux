// Tests for `src/session.ts`. Drive the ClaudeSession against a
// FakeQuery by injecting `setQueryFactoryForTests`.

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";

import type { EventEmitter } from "../src/session.ts";
import {
  ClaudeSession,
  resetInterruptExitTimeoutForTests,
  resetQueryFactoryForTests,
  setInterruptExitTimeoutForTests,
  setQueryFactoryForTests,
  type SessionStartInput,
} from "../src/session.ts";
import { asQuery, FakeQuery } from "./fake-query.ts";

interface RecordedNotification {
  method: string;
  params: unknown;
}

function recordingEmitter(): {
  emit: EventEmitter;
  events: RecordedNotification[];
} {
  const events: RecordedNotification[] = [];
  const emit: EventEmitter = {
    notification(method, params) {
      events.push({ method, params });
    },
  };
  return { emit, events };
}

function minimalInput(overrides: Partial<SessionStartInput> = {}): SessionStartInput {
  return {
    threadId: "t-1",
    cwd: "/tmp/test-cwd",
    pathToClaudeCodeExecutable: "/usr/bin/claude",
    ...overrides,
  };
}

/** Minimal assistant SDK message carrying a `session_id`, shared by
 *  the interrupt/rebuild tests. */
function mkAssistant(sessionId: string): SDKMessage {
  return {
    type: "assistant",
    message: {
      id: "m1",
      type: "message",
      role: "assistant",
      content: [],
      model: "claude-opus",
      stop_reason: null,
      stop_sequence: null,
      usage: { input_tokens: 1, output_tokens: 1 },
    },
    parent_tool_use_id: null,
    uuid: "u-1" as `${string}-${string}-${string}-${string}-${string}`,
    session_id: sessionId,
  } as unknown as SDKMessage;
}

/** An abort-like error matching the SDK's interrupt-abort shape. */
function abortLikeError(): Error {
  const err = new Error("request was aborted");
  err.name = "AbortError";
  return err;
}

let fake: FakeQuery;

beforeEach(() => {
  setQueryFactoryForTests((args) => {
    fake = new FakeQuery(args);
    return asQuery(fake);
  });
  // Keep the interrupt-exit race short so the soft-interrupt (timeout)
  // path doesn't stall the suite.
  setInterruptExitTimeoutForTests(50);
});

afterEach(() => {
  resetQueryFactoryForTests();
  resetInterruptExitTimeoutForTests();
});

test("session starts with minimal options and emits session-configured", () => {
  const { emit, events } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  expect(session.threadId).toBe("t-1");
  expect(events.find((e) => e.method === "session-configured")).toBeDefined();
  expect(fake.capturedOptions?.cwd).toBe("/tmp/test-cwd");
  expect(fake.capturedOptions?.pathToClaudeCodeExecutable).toBe(
    "/usr/bin/claude",
  );
  expect(fake.capturedOptions?.settingSources).toEqual([
    "user",
    "project",
    "local",
  ]);
  expect(fake.capturedOptions?.includePartialMessages).toBe(true);
});

test("sendTurn enqueues a user message that the fake query receives", async () => {
  const { emit } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  await session.sendTurn({ text: "hello" });
  // Allow the fake's background consumer to drain.
  await new Promise((r) => setTimeout(r, 10));
  expect(fake.capturedPrompts.length).toBe(1);
  expect(fake.capturedPrompts[0]?.type).toBe("user");
  await session.close();
});

test("sendTurn text-only path keeps content as a plain string", async () => {
  // Stage 6 regression: when no images are passed the user-message
  // content stays string-shaped so the SDK's existing string-content
  // fast-path (and any historical SDK assertions tied to it) remain
  // unchanged.
  const { emit } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  await session.sendTurn({ text: "hello" });
  await new Promise((r) => setTimeout(r, 10));
  const enqueued = fake.capturedPrompts[0];
  expect(enqueued?.type).toBe("user");
  // Bun's structural typing on the SDK's discriminated content union
  // keeps the type as `string | ContentBlock[]`. Walk the runtime
  // shape directly.
  expect(typeof (enqueued as { message: { content: unknown } }).message.content)
    .toBe("string");
  await session.close();
});

test("sendTurn with images builds a multi-block content array (image first, text last)", async () => {
  // Locked Stage 6 wire shape: each image lands as
  // `{type:"image", source:{type:"base64", media_type, data}}` and
  // the text block is appended last per Anthropic's "describe these
  // images" guidance.
  const { emit } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  await session.sendTurn({
    text: "what's in this?",
    images: [
      { mediaType: "image/png", dataBase64: "AAA" },
      { mediaType: "image/jpeg", dataBase64: "BBB" },
    ],
  });
  await new Promise((r) => setTimeout(r, 10));
  const enqueued = fake.capturedPrompts[0];
  const content = (enqueued as { message: { content: unknown } }).message
    .content as Array<{
    type: string;
    source?: { type: string; media_type: string; data: string };
    text?: string;
  }>;
  expect(Array.isArray(content)).toBe(true);
  expect(content.length).toBe(3);
  expect(content[0]?.type).toBe("image");
  expect(content[0]?.source?.type).toBe("base64");
  expect(content[0]?.source?.media_type).toBe("image/png");
  expect(content[0]?.source?.data).toBe("AAA");
  expect(content[1]?.type).toBe("image");
  expect(content[1]?.source?.media_type).toBe("image/jpeg");
  // Text comes last — Anthropic's recommended ordering for the
  // "look at these and answer" prompt pattern.
  expect(content[2]?.type).toBe("text");
  expect(content[2]?.text).toBe("what's in this?");
  await session.close();
});

test("sdk-session-id notification fires once, with the SDK message's session_id", async () => {
  const { emit, events } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  const mkAssistant = (sid: string): SDKMessage =>
    ({
      type: "assistant",
      message: {
        id: "m1",
        type: "message",
        role: "assistant",
        content: [],
        model: "claude-opus",
        stop_reason: null,
        stop_sequence: null,
        usage: { input_tokens: 1, output_tokens: 1 },
      },
      parent_tool_use_id: null,
      uuid: "u-1" as `${string}-${string}-${string}-${string}-${string}`,
      session_id: sid,
    }) as unknown as SDKMessage;
  fake.emit(mkAssistant("sdk-session-abc"));
  fake.emit(mkAssistant("sdk-session-abc"));
  fake.emit(mkAssistant("different-id-should-be-ignored"));
  await new Promise((r) => setTimeout(r, 10));
  const idEvents = events.filter((e) => e.method === "sdk-session-id");
  expect(idEvents.length).toBe(1);
  expect((idEvents[0]?.params as { sessionId: string }).sessionId).toBe(
    "sdk-session-abc",
  );
  await session.close();
});

test("fake query emitting assistant message produces sdk-message opaquely", async () => {
  const { emit, events } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  const msg: SDKMessage = {
    type: "assistant",
    message: {
      id: "m1",
      type: "message",
      role: "assistant",
      content: [],
      model: "claude-opus",
      stop_reason: null,
      stop_sequence: null,
      usage: { input_tokens: 1, output_tokens: 1 },
    },
    parent_tool_use_id: null,
    uuid: "u-1" as `${string}-${string}-${string}-${string}-${string}`,
    session_id: "s-1",
  } as unknown as SDKMessage;
  fake.emit(msg);
  await new Promise((r) => setTimeout(r, 10));
  const captured = events.find((e) => e.method === "sdk-message");
  expect(captured).toBeDefined();
  expect((captured?.params as { message: unknown })?.message).toEqual(msg);
  await session.close();
});

test("fake query emitting system.init produces sdk-message with subtype=init", async () => {
  const { emit, events } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  const msg = {
    type: "system",
    subtype: "init",
    agents: [],
    apiKeySource: "user",
    claude_code_version: "2.1.114",
    cwd: "/tmp",
    tools: [],
    mcp_servers: [],
    model: "claude-opus",
    permissionMode: "default",
    slash_commands: [],
    output_style: "default",
    skills: [],
    plugins: [],
    uuid: "u-init",
    session_id: "s-init",
  } as unknown as SDKMessage;
  fake.emit(msg);
  await new Promise((r) => setTimeout(r, 10));
  const captured = events.find((e) => e.method === "sdk-message");
  expect(captured).toBeDefined();
  const payload = captured?.params as { message: { subtype?: string } };
  expect(payload.message.subtype).toBe("init");
  await session.close();
});

test("30 different SDKMessage subtype variants are each forwarded", async () => {
  const { emit, events } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  // Construct 30 distinct minimally-plausible messages. Field shape
  // is not validated by the sidecar — it just JSON-forwards.
  const variants: SDKMessage[] = [];
  const base = { uuid: "u", session_id: "s" };
  const types = [
    "assistant",
    "user",
    "user-replay",
    "result",
    "system:init",
    "system:status",
    "system:compact_boundary",
    "system:api_retry",
    "system:local_command_output",
    "system:hook_started",
    "system:hook_progress",
    "system:hook_response",
    "system:plugin_install",
    "system:task_started",
    "system:task_updated",
    "system:task_progress",
    "system:task_notification",
    "system:session_state_changed",
    "system:notification",
    "system:files_persisted",
    "system:memory_recall",
    "system:elicitation_complete",
    "system:mirror_error",
    "auth_status",
    "stream_event",
    "tool_progress",
    "tool_use_summary",
    "rate_limit_event",
    "prompt_suggestion",
    "unknown_future_variant",
  ];
  for (const t of types) {
    if (t.startsWith("system:")) {
      const sub = t.slice("system:".length);
      variants.push({ type: "system", subtype: sub, ...base } as unknown as SDKMessage);
    } else {
      variants.push({ type: t, ...base } as unknown as SDKMessage);
    }
  }
  for (const v of variants) fake.emit(v);
  await new Promise((r) => setTimeout(r, 20));
  const sdkMessages = events.filter((e) => e.method === "sdk-message");
  expect(sdkMessages.length).toBe(variants.length);
  await session.close();
});

test("interrupt calls query.interrupt and does not hang", async () => {
  const { emit } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  await session.interrupt();
  expect(fake.interruptCalls).toBe(1);
  await session.close();
});

test("setModel calls query.setModel with the given model string", async () => {
  const { emit } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  await session.setModel("claude-opus-4-7");
  expect(fake.setModelCalls).toEqual(["claude-opus-4-7"]);
  await session.close();
});

test("setModel with undefined calls query.setModel with no arg", async () => {
  const { emit } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  await session.setModel(undefined);
  expect(fake.setModelCalls).toEqual([undefined]);
  await session.close();
});

test("setPermissionMode calls query.setPermissionMode with valid mode", async () => {
  const { emit } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  await session.setPermissionMode("plan");
  expect(fake.setPermissionModeCalls).toEqual(["plan"]);
  await session.close();
});

test("close triggers query.close and subsequent sendTurn fails cleanly", async () => {
  const { emit } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  await session.close();
  expect(fake.closed).toBe(true);
  await expect(session.sendTurn({ text: "late" })).rejects.toThrow(
    /session is closed/,
  );
});

test("session-ended notification fires when query iteration completes", async () => {
  const { emit, events } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  fake.endStream();
  // Wait for iteration task to complete.
  await new Promise((r) => setTimeout(r, 20));
  const ended = events.find(
    (e) =>
      e.method === "session-ended" &&
      (e.params as { reason: string }).reason === "iteration-complete",
  );
  expect(ended).toBeDefined();
  await session.close();
});

test("session-error notification fires when query throws non-abort", async () => {
  const { emit, events } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  fake.errorStream(new Error("boom"));
  await new Promise((r) => setTimeout(r, 20));
  const errored = events.find((e) => e.method === "session-error");
  expect(errored).toBeDefined();
  await session.close();
});

test("session-ended with reason=interrupted when query throws AbortError-like", async () => {
  const { emit, events } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  const abortErr = new Error("request was aborted");
  abortErr.name = "AbortError";
  fake.errorStream(abortErr);
  await new Promise((r) => setTimeout(r, 20));
  const ended = events.find(
    (e) =>
      e.method === "session-ended" &&
      (e.params as { reason: string }).reason === "interrupted",
  );
  expect(ended).toBeDefined();
  await session.close();
});

test("interrupt on an abort-like exit emits turn-interrupted, not session-ended", async () => {
  const { emit, events } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  // The SDK aborts the query when interrupted; model that on the fake.
  fake.interruptError = abortLikeError();
  await session.interrupt();
  expect(events.filter((e) => e.method === "session-ended")).toHaveLength(0);
  const interrupted = events.filter((e) => e.method === "turn-interrupted");
  expect(interrupted).toHaveLength(1);
  expect((interrupted[0]?.params as { threadId: string }).threadId).toBe("t-1");
  await session.close();
});

test("sendTurn after an interrupt rebuilds a resumed query that consumes the turn", async () => {
  const fakes: FakeQuery[] = [];
  setQueryFactoryForTests((args) => {
    const f = new FakeQuery(args);
    fakes.push(f);
    return asQuery(f);
  });
  const { emit } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  // Observe an sdk session id on the original query.
  fakes[0]?.emit(mkAssistant("sdk-sess-1"));
  await new Promise((r) => setTimeout(r, 10));
  // Interrupt kills the original query.
  const first = fakes[0] as FakeQuery;
  first.interruptError = abortLikeError();
  await session.interrupt();
  // The next turn transparently rebuilds a resumed query.
  await session.sendTurn({ text: "resumed turn" });
  await new Promise((r) => setTimeout(r, 10));
  expect(fakes.length).toBe(2);
  // The rebuild resumes from the previously observed sdk session id and
  // drops the explicit fresh-session uuid.
  expect(fakes[1]?.capturedOptions?.resume).toBe("sdk-sess-1");
  expect(fakes[1]?.capturedOptions?.sessionId).toBeUndefined();
  // The message lands on the NEW query, not the dead one.
  expect(fakes[1]?.capturedPrompts.length).toBe(1);
  expect(fakes[0]?.capturedPrompts.length).toBe(0);
  await session.close();
});

test("rebuilt query re-emits sdk-session-id when its first message carries a new id", async () => {
  const fakes: FakeQuery[] = [];
  setQueryFactoryForTests((args) => {
    const f = new FakeQuery(args);
    fakes.push(f);
    return asQuery(f);
  });
  const { emit, events } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  fakes[0]?.emit(mkAssistant("sdk-sess-1"));
  await new Promise((r) => setTimeout(r, 10));
  const first = fakes[0] as FakeQuery;
  first.interruptError = abortLikeError();
  await session.interrupt();
  await session.sendTurn({ text: "again" });
  // The resumed session is issued a fresh id.
  fakes[1]?.emit(mkAssistant("sdk-sess-2"));
  await new Promise((r) => setTimeout(r, 10));
  const ids = events
    .filter((e) => e.method === "sdk-session-id")
    .map((e) => (e.params as { sessionId: string }).sessionId);
  expect(ids).toEqual(["sdk-sess-1", "sdk-sess-2"]);
  await session.close();
});

test("setModel/setPermissionMode while the query is dead don't throw and are reflected on rebuild", async () => {
  const fakes: FakeQuery[] = [];
  setQueryFactoryForTests((args) => {
    const f = new FakeQuery(args);
    fakes.push(f);
    return asQuery(f);
  });
  const { emit } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  const first = fakes[0] as FakeQuery;
  first.interruptError = abortLikeError();
  await session.interrupt();
  // Query is dead — these record without touching the dead query.
  await session.setModel("claude-opus-4-8");
  await session.setPermissionMode("plan");
  expect(fakes[0]?.setModelCalls).toEqual([]);
  expect(fakes[0]?.setPermissionModeCalls).toEqual([]);
  await session.sendTurn({ text: "go" });
  await new Promise((r) => setTimeout(r, 10));
  expect(fakes[1]?.capturedOptions?.model).toBe("claude-opus-4-8");
  expect(fakes[1]?.capturedOptions?.permissionMode).toBe("plan");
  await session.close();
});

test("interrupt with a soft (non-exiting) iterator resolves without turn-interrupted and stays usable", async () => {
  const { emit, events } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  // interruptError stays null — the fake's iterator keeps running, so
  // the interrupt-exit race resolves via the (lowered) timeout.
  await session.interrupt();
  expect(fake.interruptCalls).toBe(1);
  expect(events.some((e) => e.method === "turn-interrupted")).toBe(false);
  // Session is still usable and did NOT rebuild (query still live).
  await session.sendTurn({ text: "still here" });
  await new Promise((r) => setTimeout(r, 10));
  expect(fake.capturedPrompts.length).toBe(1);
  await session.close();
});

test("closing one session does not affect another session", async () => {
  const fakes: FakeQuery[] = [];
  setQueryFactoryForTests((args) => {
    const f = new FakeQuery(args);
    fakes.push(f);
    return asQuery(f);
  });
  const { emit: e1 } = recordingEmitter();
  const { emit: e2 } = recordingEmitter();
  const s1 = new ClaudeSession(minimalInput({ threadId: "ta" }), e1);
  const s2 = new ClaudeSession(minimalInput({ threadId: "tb" }), e2);
  await s1.close();
  expect(fakes[0]?.closed).toBe(true);
  expect(fakes[1]?.closed).toBe(false);
  await s2.sendTurn({ text: "still alive" });
  await new Promise((r) => setTimeout(r, 10));
  expect(fakes[1]?.capturedPrompts.length).toBe(1);
  await s2.close();
});

test("initializationResult forwards the SDK response opaquely", async () => {
  const { emit } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  const payload = {
    commands: [{ name: "test", description: "x", argumentHint: "" }],
    agents: [],
    output_style: "default",
    available_output_styles: ["default"],
    models: [],
    account: { email: "a@b.c" },
  };
  fake.initResult = payload;
  const got = await session.initializationResult();
  expect(got).toEqual(payload);
  await session.close();
});

test("AskUserQuestion tool_use does NOT emit a user-input-requested side-channel", async () => {
  // Stage 1 dedup: the canUseTool permission bridge now carries the
  // AskUserQuestion request through `request-opened` with
  // `kind: "user-input"`. Firing an additional side-channel here
  // produced duplicate cards with mismatching request ids.
  const { emit, events } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  const msg = {
    type: "assistant",
    message: {
      content: [
        {
          type: "tool_use",
          id: "tu1",
          name: "AskUserQuestion",
          input: { questions: [{ prompt: "color?" }] },
        },
      ],
    },
    parent_tool_use_id: null,
  } as unknown as SDKMessage;
  fake.emit(msg);
  await new Promise((r) => setTimeout(r, 10));
  // The raw sdk-message still fires — the UI derives the tool_call
  // row from it. The side-channel is gone.
  expect(events.some((e) => e.method === "sdk-message")).toBe(true);
  expect(events.some((e) => e.method === "user-input-requested")).toBe(false);
  await session.close();
});

test("ExitPlanMode tool_use emits a plan-proposed side-channel", async () => {
  const { emit, events } = recordingEmitter();
  const session = new ClaudeSession(minimalInput(), emit);
  const msg = {
    type: "assistant",
    message: {
      content: [
        {
          type: "tool_use",
          id: "tu-plan",
          name: "ExitPlanMode",
          input: { plan: "step one" },
        },
      ],
    },
    parent_tool_use_id: null,
  } as unknown as SDKMessage;
  fake.emit(msg);
  await new Promise((r) => setTimeout(r, 10));
  const plan = events.find((e) => e.method === "plan-proposed");
  expect(plan).toBeDefined();
  expect((plan?.params as { plan: string }).plan).toBe("step one");
  await session.close();
});

test("effort option is passed through opaquely even for future values", () => {
  const { emit } = recordingEmitter();
  const session = new ClaudeSession(
    minimalInput({ effort: "xhigh" }),
    emit,
  );
  expect((fake.capturedOptions as { effort?: string })?.effort).toBe("xhigh");
  void session;
});

test("allowDangerouslySkipPermissions only set when permissionMode=bypass", () => {
  const { emit } = recordingEmitter();
  new ClaudeSession(
    minimalInput({
      permissionMode: "bypassPermissions",
      allowDangerouslySkipPermissions: true,
    }),
    emit,
  );
  expect(fake.capturedOptions?.allowDangerouslySkipPermissions).toBe(true);

  setQueryFactoryForTests((args) => {
    fake = new FakeQuery(args);
    return asQuery(fake);
  });
  const { emit: e2 } = recordingEmitter();
  new ClaudeSession(
    minimalInput({ permissionMode: "default" }),
    e2,
  );
  expect(fake.capturedOptions?.allowDangerouslySkipPermissions).toBeUndefined();
});

// ────────────────────────────────────────────────────────────────────
// Bypass-downgrade restore. The CLI can silently boot a
// `bypassPermissions` launch in `default` mode when its consent read
// loses a race with concurrent settings writers, which makes the SDK
// call `canUseTool`. The session's canUseTool context auto-allows and
// fires a single live `setPermissionMode("bypassPermissions")` restore.
// Drive the captured `canUseTool` directly.
// ────────────────────────────────────────────────────────────────────

/** A never-aborting callbackOptions shim for the captured canUseTool. */
function canUseToolOptions() {
  return {
    signal: new AbortController().signal,
    toolUseID: "tu-restore",
  } as unknown as Parameters<
    NonNullable<import("@anthropic-ai/claude-agent-sdk").Options["canUseTool"]>
  >[2];
}

test("intended-bypass downgrade auto-allows and attempts a single live restore", async () => {
  const { emit } = recordingEmitter();
  new ClaudeSession(
    minimalInput({
      permissionMode: "bypassPermissions",
      allowDangerouslySkipPermissions: true,
    }),
    emit,
  );
  const canUseTool = fake.capturedOptions?.canUseTool;
  expect(canUseTool).toBeDefined();

  // Three tool calls arrive while the session still intends bypass.
  const r1 = await canUseTool!("Bash", { command: "ls" }, canUseToolOptions());
  const r2 = await canUseTool!("Edit", { path: "/x" }, canUseToolOptions());
  const r3 = await canUseTool!("Bash", { command: "pwd" }, canUseToolOptions());

  // Every call is auto-allowed with its original input.
  expect(r1.behavior).toBe("allow");
  expect(r2.behavior).toBe("allow");
  expect(r3.behavior).toBe("allow");
  if (r1.behavior === "allow") expect(r1.updatedInput).toEqual({ command: "ls" });

  // Only the FIRST call fires the live restore; the one-shot guard
  // suppresses the rest.
  expect(fake.setPermissionModeCalls).toEqual(["bypassPermissions"]);
});

test("downgrade restore re-arms after ensureLiveQuery rebuilds the query", async () => {
  const { emit } = recordingEmitter();
  const session = new ClaudeSession(
    minimalInput({
      permissionMode: "bypassPermissions",
      allowDangerouslySkipPermissions: true,
    }),
    emit,
  );
  const firstQuery = fake;
  const canUseTool = firstQuery.capturedOptions?.canUseTool;

  await canUseTool!("Bash", { command: "ls" }, canUseToolOptions());
  await canUseTool!("Bash", { command: "ls" }, canUseToolOptions());
  expect(firstQuery.setPermissionModeCalls).toEqual(["bypassPermissions"]);

  // Emit a session id, interrupt to kill the query, then send a turn
  // to trigger ensureLiveQuery rebuilding a fresh (resumed) query.
  firstQuery.emit(mkAssistant("sess-1"));
  await new Promise((r) => setTimeout(r, 10));
  firstQuery.interruptError = abortLikeError();
  await session.interrupt();
  await session.sendTurn({ text: "again" });
  await new Promise((r) => setTimeout(r, 10));

  // `fake` now points at the rebuilt query. Its canUseTool gets a fresh
  // one-shot restore budget.
  const rebuilt = fake;
  expect(rebuilt).not.toBe(firstQuery);
  const rebuiltCanUseTool = rebuilt.capturedOptions?.canUseTool;
  await rebuiltCanUseTool!("Bash", { command: "ls" }, canUseToolOptions());
  await rebuiltCanUseTool!("Bash", { command: "ls" }, canUseToolOptions());
  expect(rebuilt.setPermissionModeCalls).toEqual(["bypassPermissions"]);

  await session.close();
});
