import { beforeEach, describe, expect, it } from "vitest";

import type { ProviderRuntimeEvent } from "@/tauri/events";

import {
  formatClockTime,
  formatGoalAge,
  goalLastActivity,
  parseGoalCommand,
  remapGoalSource,
  resolveThreadGoal,
} from "./goal";
import { replayPayloads } from "./hydrate";
import {
  __resetReducerIdCounterForTests,
  appendUserMessage,
  applyEvent,
  createEmptyThreadState,
  removeUserMessageByNonce,
} from "./reducer";
import type { ChatViewItem } from "./types";

const at = (ms: number) => () => ms;

function userRow(text: string, nonce?: string): string {
  return JSON.stringify({
    type: "user_message",
    thread_id: "t1",
    text,
    ...(nonce ? { client_nonce: nonce } : {}),
  });
}

function completedRow(turnId: string): string {
  const event: ProviderRuntimeEvent = {
    type: "turn_completed",
    thread_id: "t1",
    turn_id: turnId,
    status: { kind: "success" },
    usage: null,
  };
  return JSON.stringify(event);
}

beforeEach(() => {
  __resetReducerIdCounterForTests();
});

describe("parseGoalCommand", () => {
  it.each<[string, ReturnType<typeof parseGoalCommand>]>([
    ["/goal ship the importer", { kind: "set", text: "ship the importer" }],
    ["  /GOAL  keep\nthe newline  ", { kind: "set", text: "keep\nthe newline" }],
    ["/goal clear", { kind: "clear" }],
    ["/goal Clear", { kind: "clear" }],
    ["/goal resume", { kind: "control" }],
    ["/goal pause", { kind: "control" }],
    ["/goal status", { kind: "control" }],
    ["/goal", { kind: "control" }],
    ["/goals list", null],
    ["set a /goal later", null],
    ["Continue working toward this goal: ship it", null],
  ])("%j", (text, expected) => {
    expect(parseGoalCommand(text)).toEqual(expected);
  });
});

describe("goal state in the reducer", () => {
  it("records the goal on the optimistic send, before any provider event", () => {
    const state = appendUserMessage(
      createEmptyThreadState(),
      "/goal migrate the importer",
      at(1_000),
      "n1",
    );
    expect(state.goal).toEqual({
      text: "migrate the importer",
      setAt: 1_000,
      sourceMessageId: state.messages[0]!.id,
    });
    expect(state.goalHistory).toEqual([]);
  });

  it("ignores ordinary turns and control subcommands", () => {
    let state = appendUserMessage(createEmptyThreadState(), "/goal first", at(1));
    const goal = state.goal;
    state = appendUserMessage(state, "hello", at(2));
    state = appendUserMessage(state, "/goal resume", at(3));
    state = appendUserMessage(state, "/goal status", at(4));
    expect(state.goal).toBe(goal);
    expect(state.goalHistory).toEqual([]);
  });

  it("a later /goal replaces the snapshot and keeps the old one in history", () => {
    let state = appendUserMessage(createEmptyThreadState(), "/goal first", at(1));
    const first = state.goal;
    state = appendUserMessage(state, "/goal second", at(2));
    expect(state.goal?.text).toBe("second");
    expect(state.goalHistory).toEqual([first]);
  });

  it("/goal clear removes the goal", () => {
    let state = appendUserMessage(createEmptyThreadState(), "/goal first", at(1));
    state = appendUserMessage(state, "/goal clear", at(2));
    expect(state.goal).toBeNull();
    expect(state.goalHistory.map((g) => g.text)).toEqual(["first"]);
  });

  it("records once when the persisted row echoes the optimistic send", () => {
    let state = appendUserMessage(createEmptyThreadState(), "/goal a", at(1));
    state = appendUserMessage(state, "/goal b", at(2), "n2");
    const recorded = state.goal;
    state = applyEvent(state, {
      type: "user_message",
      thread_id: "t1",
      text: "/goal b",
      client_nonce: "n2",
    });
    expect(state.goal).toBe(recorded);
    expect(state.goalHistory.map((g) => g.text)).toEqual(["a"]);
  });

  it("a failed /goal send rolls the goal back with its bubble", () => {
    let state = appendUserMessage(createEmptyThreadState(), "/goal a", at(1));
    state = appendUserMessage(state, "/goal b", at(2), "n2");
    state = removeUserMessageByNonce(state, "n2");
    expect(state.goal?.text).toBe("a");
    expect(state.goalHistory).toEqual([]);

    state = appendUserMessage(state, "/goal clear", at(3), "n3");
    expect(state.goal).toBeNull();
    state = removeUserMessageByNonce(state, "n3");
    expect(state.goal?.text).toBe("a");
  });
});

describe("goal across a restart", () => {
  it("quit between turns: the goal comes back standing", () => {
    const state = replayPayloads([
      userRow("/goal ship the importer"),
      completedRow("turn-1"),
    ]);
    expect(resolveThreadGoal(state.goal, state.interrupted)?.status).toBe(
      "standing",
    );
    expect(state.goal?.text).toBe("ship the importer");
  });

  it("quit mid-turn: the goal comes back interrupted", () => {
    const state = replayPayloads([
      userRow("/goal ship the importer"),
      completedRow("turn-1"),
      userRow("keep going"),
    ]);
    expect(resolveThreadGoal(state.goal, state.interrupted)?.status).toBe(
      "interrupted",
    );
  });

  it("a live run on remount is not an interruption", () => {
    const state = replayPayloads([userRow("/goal ship the importer")], {
      runLive: true,
    });
    expect(resolveThreadGoal(state.goal, state.interrupted)?.status).toBe(
      "standing",
    );
  });

  it("follows the source turn onto an adopted id", () => {
    const replayed: ChatViewItem[] = [
      { kind: "user_message", id: "user-9", seq: 0, text: "/goal x" },
    ];
    const adopted: ChatViewItem[] = [{ ...replayed[0]!, id: "user-1" }];
    const goal = { text: "x", setAt: 1, sourceMessageId: "user-9" };
    expect(remapGoalSource(goal, replayed, adopted)?.sourceMessageId).toBe(
      "user-1",
    );
    expect(remapGoalSource(goal, replayed, replayed)).toBe(goal);
  });
});

describe("formatGoalAge", () => {
  it.each([
    [0, "<1m"],
    [59_000, "<1m"],
    [4 * 60_000, "4m"],
    [3 * 3_600_000 + 5, "3h"],
    [50 * 3_600_000, "2d"],
  ])("%d ms → %s", (ms, label) => {
    expect(formatGoalAge(ms)).toBe(label);
  });
});

describe("goalLastActivity", () => {
  it("reads the newest timestamp and the last reply's first sentence", () => {
    const messages: ChatViewItem[] = [
      { kind: "user_message", id: "u1", seq: 0, text: "/goal x", created_at: 1_000 },
      {
        kind: "assistant_message",
        id: "a1",
        seq: 1,
        turn_id: "t1",
        text: "Pinned the renderer scale factor in `window-scale.ts`. Next I'll sweep the text sizes.",
        streaming: false,
      } as ChatViewItem,
      { kind: "user_message", id: "u2", seq: 2, text: "keep going", created_at: 5_000 },
    ];
    expect(goalLastActivity(messages)).toEqual({
      at: 5_000,
      itemId: "a1",
      turnId: "t1",
      snippet: "Pinned the renderer scale factor in window-scal…",
    });
  });

  it("returns null for an empty transcript", () => {
    expect(goalLastActivity([])).toBeNull();
  });
});

describe("formatClockTime", () => {
  it("renders a 24-hour HH:MM", () => {
    expect(formatClockTime(new Date(2026, 8, 14, 9, 5).getTime())).toMatch(/^09:05$/);
  });
});
