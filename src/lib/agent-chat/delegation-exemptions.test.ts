import { beforeEach, describe, expect, it } from "vitest";

import type { ProviderRuntimeEvent, SubagentSnapshot } from "@/tauri/events";

import { replayPayloads } from "./hydrate";
import {
  __resetReducerIdCounterForTests,
  applyEvent,
  createEmptyThreadState,
} from "./reducer";
import {
  countRunningSubagents,
  delegationElapsedLabel,
  interruptRunningSubagents,
  mergeSnapshot,
  newSubagentView,
  runningDelegations,
  runningSubagentEntries,
  subagentElapsedMs,
} from "./subagents";
import { shouldShowThinkingIndicator } from "./thinking";
import type {
  ChatThreadState,
  ChatViewItem,
  SubagentRunItem,
  SubagentView,
  TurnEndedItem,
} from "./types";

/**
 * Cross-provider delegations ride on the parent's subagent rows but are not
 * the parent's work: the backend owns their settle, so every frontend rule
 * that infers a subagent's end or holds the parent's turn must skip them.
 */

const T = "parent";

function snap(
  subagent: Partial<SubagentSnapshot> & Pick<SubagentSnapshot, "subagent_id" | "status">,
): ProviderRuntimeEvent {
  return { type: "subagent_updated", thread_id: T, subagent };
}

const delegated = (status: SubagentSnapshot["status"]) =>
  snap({
    subagent_id: "delegate:child-1",
    name: "Codex",
    agent_type: "codex",
    description: "Add slugify helper",
    model: "gpt-6.1-codex",
    effort: "high",
    task_kind: "agent",
    provider_ref: "child-1",
    status,
    activity: status === "running" ? "Working in its tab" : null,
  });

const running: ProviderRuntimeEvent = {
  type: "session_state_changed",
  thread_id: T,
  status: { status: "running", active_turn: "turn-1" },
};
const completed: ProviderRuntimeEvent = {
  type: "turn_completed",
  thread_id: T,
  turn_id: "turn-1",
  status: { kind: "success" },
  usage: null,
};
const user = (text: string): ProviderRuntimeEvent => ({
  type: "user_message",
  thread_id: T,
  text,
});

function run(events: ProviderRuntimeEvent[], initial?: ChatThreadState): ChatThreadState {
  return events.reduce<ChatThreadState>(
    (state, event) => applyEvent(state, event, () => 1_000),
    initial ?? createEmptyThreadState(),
  );
}

function cards(state: ChatThreadState): SubagentRunItem[] {
  return state.messages.filter(
    (item): item is SubagentRunItem => item.kind === "subagent_run",
  );
}

function view(state: ChatThreadState, id: string): SubagentView | undefined {
  for (const card of cards(state)) {
    const found = card.subagents.find((sub) => sub.id === id);
    if (found) return found;
  }
  return undefined;
}

beforeEach(() => {
  __resetReducerIdCounterForTests();
});

describe("delegated rows in the subagent helpers", () => {
  const delegatedView: SubagentView = {
    ...newSubagentView("delegate:child-1", 0),
    status: "running",
  };
  const nativeView: SubagentView = { ...newSubagentView("explore", 0), status: "running" };
  const messages: ChatViewItem[] = [
    { kind: "subagent_run", id: "c1", seq: 0, turn_id: "t1", subagents: [delegatedView] },
    { kind: "subagent_run", id: "c2", seq: 1, turn_id: "t1", subagents: [nativeView] },
  ];

  it("copies effort from the wire", () => {
    const merged = mergeSnapshot(newSubagentView("delegate:x", 0), {
      subagent_id: "delegate:x",
      status: "running",
      effort: "max",
    });
    expect(merged.effort).toBe("max");
  });

  it("never forces a delegated row to interrupted", () => {
    const next = interruptRunningSubagents(messages, 5);
    const [first, second] = next as SubagentRunItem[];
    expect(first.subagents[0].status).toBe("running");
    expect(second.subagents[0].status).toBe("interrupted");
  });

  it("never counts a delegated row as the parent's background work", () => {
    expect(countRunningSubagents(messages, true)).toBe(1);
    expect(runningSubagentEntries(messages, true).map((e) => e.subagent.id)).toEqual([
      "explore",
    ]);
  });

  it("lists running delegations on their own", () => {
    expect(runningDelegations(messages).map((v) => v.id)).toEqual(["delegate:child-1"]);
    const settled = messages.map((item) =>
      item.kind === "subagent_run"
        ? { ...item, subagents: item.subagents.map((s) => ({ ...s, status: "completed" as const })) }
        : item,
    );
    expect(runningDelegations(settled)).toEqual([]);
  });

  it("shows no elapsed for a delegation that ended without the backend's duration", () => {
    // Codemux closed mid-task: the relaunch's Stopped card has no duration,
    // and start-to-settle would count the hours it was closed.
    const closed: SubagentView = {
      ...newSubagentView("delegate:child-1", 1_000),
      status: "stopped",
      resultText: "Codemux closed before this task finished.",
      finishedAt: 3_600_000,
    };
    expect(subagentElapsedMs(closed, 4_000_000)).toBeNull();
    expect(delegationElapsedLabel([closed], 4_000_000)).toBe("");
    expect(subagentElapsedMs({ ...closed, durationMs: 9_000 }, 4_000_000)).toBe(9_000);
    // A native row still freezes at its settle stamp.
    expect(subagentElapsedMs({ ...closed, id: "explore" }, 4_000_000)).toBe(3_599_000);
  });

  it("does not hold the thinking pulse back for a delegated tail card", () => {
    expect(shouldShowThinkingIndicator(messages.slice(0, 1), true)).toBe(true);
    expect(shouldShowThinkingIndicator(messages.slice(1), true)).toBe(false);
  });
});

describe("delegated rows in the reducer", () => {
  it("lets the parent's turn settle while a delegated task runs", () => {
    const state = run([user("delegate it"), running, delegated("pending"), delegated("running"), completed]);
    expect(state.streaming).toBe(false);
    const ended = state.messages.find(
      (item): item is TurnEndedItem => item.kind === "turn_ended",
    );
    expect(ended?.interim).toBeUndefined();
    expect(view(state, "delegate:child-1")?.status).toBe("running");
  });

  it("still holds the turn for a native subagent (control)", () => {
    const state = run([
      user("explore"),
      running,
      snap({ subagent_id: "explore", status: "running" }),
      completed,
    ]);
    expect(state.streaming).toBe(true);
  });

  it("keeps a delegated row running across the next user turn and a session close", () => {
    let state = run([user("delegate it"), running, delegated("running"), completed]);
    state = run([user("meanwhile, something else")], state);
    expect(view(state, "delegate:child-1")?.status).toBe("running");
    state = run(
      [{ type: "session_state_changed", thread_id: T, status: { status: "closed" } }],
      state,
    );
    expect(view(state, "delegate:child-1")?.status).toBe("running");
  });

  it("settles only on the backend's terminal snapshot, with the full field set", () => {
    const state = run([
      user("delegate it"),
      running,
      delegated("running"),
      completed,
      snap({
        subagent_id: "delegate:child-1",
        name: "Codex",
        agent_type: "codex",
        description: "Add slugify helper",
        model: "gpt-6.1-codex",
        effort: "high",
        status: "completed",
        result_text: "Added slugify.",
        duration_ms: 372_000,
      }),
    ]);
    expect(view(state, "delegate:child-1")).toMatchObject({
      status: "completed",
      resultText: "Added slugify.",
      durationMs: 372_000,
      effort: "high",
    });
  });

  it("never shares a card between delegated and native rows", () => {
    const state = run([
      user("both"),
      running,
      snap({ subagent_id: "explore", status: "running" }),
      delegated("pending"),
      snap({ subagent_id: "verify", status: "running" }),
    ]);
    expect(cards(state).map((card) => card.subagents.map((sub) => sub.id))).toEqual([
      ["explore"],
      ["delegate:child-1"],
      ["verify"],
    ]);
  });

  it("groups back-to-back delegations in one card", () => {
    const state = run([
      user("two agents"),
      running,
      delegated("pending"),
      snap({ subagent_id: "delegate:child-2", agent_type: "claude", status: "pending" }),
    ]);
    expect(cards(state).map((card) => card.subagents.map((sub) => sub.id))).toEqual([
      ["delegate:child-1", "delegate:child-2"],
    ]);
  });

  it("leaves a still-running delegation alone on a cold hydrate", () => {
    const payloads = [user("delegate it"), running, delegated("running"), completed].map((event) =>
      JSON.stringify(event),
    );
    const state = replayPayloads(payloads);
    expect(view(state, "delegate:child-1")?.status).toBe("running");
  });
});
