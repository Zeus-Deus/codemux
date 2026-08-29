// Tests for `src/methods/list-sessions.ts` and its registry entry.
//
// The SDK's session-history read is swapped out through the
// `setSessionListerForTests` seam — the same DI pattern `session.ts`
// uses for `query()` — so nothing here touches the real filesystem or
// the user's actual conversation history.

import { afterEach, describe, expect, test } from "bun:test";
import type { SDKSessionInfo } from "@anthropic-ai/claude-agent-sdk";

import {
  buildMethods,
  InvalidParamsError,
  type MethodHandler,
} from "../src/methods/index.ts";
import type { EventEmitter } from "../src/session.ts";
import {
  listSessions,
  resetSessionListerForTests,
  setSessionListerForTests,
  type ListSessionsResult,
} from "../src/methods/list-sessions.ts";

const noopEmitter: EventEmitter = {
  notification() {},
};

function registryMethod(name: string): MethodHandler {
  const handler = buildMethods(noopEmitter)[name];
  if (!handler) throw new Error(`no registered method named ${name}`);
  return handler;
}

/** Overrides may blank a field out with `undefined`; `sdkSession`
 *  strips those keys so the row matches what the SDK really returns for
 *  absent metadata under `exactOptionalPropertyTypes`. */
type SessionOverrides = {
  [K in keyof SDKSessionInfo]?: SDKSessionInfo[K] | undefined;
};

/** Minimal SDK row. Overrides layer on the interesting fields. */
function sdkSession(overrides: SessionOverrides = {}): SDKSessionInfo {
  const row: SessionOverrides = {
    sessionId: "11111111-2222-3333-4444-555555555555",
    summary: "Wire up the exporter",
    lastModified: Date.parse("2026-08-20T10:30:00.000Z"),
    cwd: "/home/dev/projects/app",
    ...overrides,
  };
  for (const key of Object.keys(row) as Array<keyof SessionOverrides>) {
    if (row[key] === undefined) delete row[key];
  }
  return row as SDKSessionInfo;
}

/** Install a canned history and record the options the method sent. */
function stubHistory(rows: SDKSessionInfo[]): {
  calls: Array<{ dir?: string; includeWorktrees: boolean; limit: number }>;
} {
  const calls: Array<{
    dir?: string;
    includeWorktrees: boolean;
    limit: number;
  }> = [];
  setSessionListerForTests(async (options) => {
    calls.push(options);
    return rows;
  });
  return { calls };
}

afterEach(() => {
  resetSessionListerForTests();
});

// ---------------------------------------------------------------------------
// Param validation
// ---------------------------------------------------------------------------

describe("list-sessions param validation", () => {
  test("rejects a non-object payload", async () => {
    stubHistory([]);
    const method = registryMethod("list-sessions");
    for (const bad of [undefined, null, "sessions", 7, ["a"]]) {
      await expect(method(bad)).rejects.toBeInstanceOf(InvalidParamsError);
    }
  });

  test("rejects wrongly typed optional fields", async () => {
    stubHistory([]);
    const method = registryMethod("list-sessions");
    await expect(method({ dir: 12 })).rejects.toThrow(
      "dir must be a string when present",
    );
    await expect(method({ includeWorktrees: "yes" })).rejects.toThrow(
      "includeWorktrees must be a boolean when present",
    );
    await expect(method({ limit: "200" })).rejects.toThrow(
      "limit must be a number when present",
    );
    await expect(method({ limit: Number.NaN })).rejects.toThrow(
      "limit must be a number when present",
    );
  });

  test("an empty payload uses the shipped defaults and omits dir", async () => {
    const { calls } = stubHistory([]);
    await registryMethod("list-sessions")({});
    expect(calls).toEqual([{ includeWorktrees: true, limit: 200 }]);
    expect("dir" in (calls[0] as object)).toBe(false);
  });

  test("supplied scope is forwarded verbatim", async () => {
    const { calls } = stubHistory([]);
    await registryMethod("list-sessions")({
      dir: "/home/dev/projects/app",
      includeWorktrees: false,
      limit: 25,
    });
    expect(calls[0]).toEqual({
      dir: "/home/dev/projects/app",
      includeWorktrees: false,
      limit: 25,
    });
  });

  test("a non-positive or fractional limit is clamped before the SDK", async () => {
    const { calls } = stubHistory([]);
    await listSessions({ includeWorktrees: true, limit: 0 });
    await listSessions({ includeWorktrees: true, limit: -5 });
    await listSessions({ includeWorktrees: true, limit: 12.7 });
    expect(calls.map((c) => c.limit)).toEqual([1, 1, 12]);
  });
});

// ---------------------------------------------------------------------------
// Title resolution
// ---------------------------------------------------------------------------

describe("list-sessions title resolution", () => {
  async function titleOf(
    overrides: SessionOverrides,
  ): Promise<{ title: string; titleSource: string }> {
    stubHistory([sdkSession(overrides)]);
    const result = (await listSessions({
      includeWorktrees: true,
      limit: 10,
    })) as ListSessionsResult;
    const first = result.sessions[0];
    if (!first) throw new Error("expected one session");
    return { title: first.title, titleSource: first.titleSource };
  }

  test("a custom title wins over every other candidate", async () => {
    expect(
      await titleOf({
        customTitle: "Renamed by hand",
        summary: "Auto summary",
        firstPrompt: "the first thing I asked",
      }),
    ).toEqual({ title: "Renamed by hand", titleSource: "custom" });
  });

  test("the summary is used when there is no custom title", async () => {
    expect(
      await titleOf({
        summary: "Auto summary",
        firstPrompt: "the first thing I asked",
      }),
    ).toEqual({ title: "Auto summary", titleSource: "summary" });
  });

  test("the first prompt is used when the summary is blank", async () => {
    expect(
      await titleOf({
        customTitle: "   ",
        summary: "",
        firstPrompt: "the first thing I asked",
      }),
    ).toEqual({ title: "the first thing I asked", titleSource: "prompt" });
  });

  test("with nothing at all it falls back to the folder plus a short id", async () => {
    expect(
      await titleOf({
        sessionId: "abcdef01-2222-3333-4444-555555555555",
        summary: "",
        cwd: "/home/dev/projects/app/",
      }),
    ).toEqual({ title: "app · abcdef01", titleSource: "fallback" });
  });

  test("multi-line titles collapse to a single line", async () => {
    expect(
      (await titleOf({ summary: "  fix the\n\n  parser  crash \t" })).title,
    ).toBe("fix the parser crash");
  });

  test("an overlong title is truncated with an ellipsis", async () => {
    const { title } = await titleOf({ summary: "x".repeat(500) });
    expect(title.length).toBe(200);
    expect(title.endsWith("…")).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Noise filtering and field normalization
// ---------------------------------------------------------------------------

describe("list-sessions filtering", () => {
  test("temp-directory sessions are dropped without being counted as skipped", async () => {
    stubHistory([
      sdkSession({ sessionId: "keep-1", cwd: "/home/dev/projects/app" }),
      sdkSession({ sessionId: "drop-1", cwd: "/tmp/pytest-of-dev/test_run0" }),
      sdkSession({ sessionId: "drop-2", cwd: "/tmp" }),
      sdkSession({ sessionId: "drop-3", cwd: "/private/tmp/fixture" }),
      sdkSession({ sessionId: "drop-4", cwd: "/var/tmp/scratch" }),
      // Not a temp path — `/tmp` must match whole segments only.
      sdkSession({ sessionId: "keep-2", cwd: "/home/dev/tmpfiles" }),
    ]);
    const result = await listSessions({ includeWorktrees: true, limit: 50 });
    expect(result.sessions.map((s) => s.sessionId)).toEqual([
      "keep-1",
      "keep-2",
    ]);
    expect(result.skippedWithoutCwd).toBe(0);
  });

  test("rows with no usable cwd are dropped and counted", async () => {
    stubHistory([
      sdkSession({ sessionId: "keep-1" }),
      sdkSession({ sessionId: "drop-1", cwd: undefined }),
      sdkSession({ sessionId: "drop-2", cwd: "   " }),
    ]);
    const result = await listSessions({ includeWorktrees: true, limit: 50 });
    expect(result.sessions.map((s) => s.sessionId)).toEqual(["keep-1"]);
    expect(result.skippedWithoutCwd).toBe(2);
  });

  test("timestamps, branch and size are normalized for the Rust side", async () => {
    stubHistory([
      sdkSession({
        lastModified: Date.parse("2026-08-20T10:30:00.000Z"),
        createdAt: Date.parse("2026-08-19T08:00:00.000Z"),
        gitBranch: " feature/adopt ",
        fileSize: 4096,
      }),
    ]);
    const [session] = (
      await listSessions({ includeWorktrees: true, limit: 10 })
    ).sessions;
    expect(session).toMatchObject({
      lastModified: "2026-08-20T10:30:00.000Z",
      createdAt: "2026-08-19T08:00:00.000Z",
      gitBranch: "feature/adopt",
      fileSize: 4096,
    });
  });

  test("absent optional metadata becomes null / zero rather than undefined", async () => {
    stubHistory([
      sdkSession({
        createdAt: undefined,
        gitBranch: undefined,
        fileSize: undefined,
      }),
    ]);
    const [session] = (
      await listSessions({ includeWorktrees: true, limit: 10 })
    ).sessions;
    expect(session?.createdAt).toBeNull();
    expect(session?.gitBranch).toBeNull();
    expect(session?.fileSize).toBe(0);
  });

  test("an unusable lastModified sorts to the epoch instead of throwing", async () => {
    stubHistory([sdkSession({ lastModified: Number.NaN })]);
    const [session] = (
      await listSessions({ includeWorktrees: true, limit: 10 })
    ).sessions;
    expect(session?.lastModified).toBe("1970-01-01T00:00:00.000Z");
  });
});
