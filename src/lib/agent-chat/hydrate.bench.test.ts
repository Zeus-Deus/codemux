import { describe, expect, it } from "vitest";

import type { ProviderRuntimeEvent, SubagentSnapshot } from "@/tauri/events";

import { __resetReducerIdCounterForTests } from "./reducer";
import { replayTimed, type TimedReplayPayload } from "./hydrate";
import { transcriptIndex } from "./subagents";

/**
 * Cold-hydrate benchmark + regression guard.
 *
 * Builds a synthetic persisted transcript shaped like the real long threads
 * users switch between (a 5k–11k row thread from the CPU profile that
 * motivated this: ~2600 tool_use/tool_result pairs, ~950 thinking, ~550
 * assistant text, 15 user turns, ~3600 `subagent_updated` ticks with a
 * handful of subagents still running at the end) and times `replayTimed`
 * — the exact fold `agent-chat-store.hydrateThread` runs on the main
 * thread when a big thread is visited cold.
 *
 * CI runs a single small fold (fast, only asserts it completes and stays
 * deterministic). Set `CODEMUX_BENCH=1` to run the full-size fixture with
 * several timed iterations and print the numbers:
 *
 *   CODEMUX_BENCH=1 npx vitest run src/lib/agent-chat/hydrate.bench.test.ts
 */

export interface BenchFixtureShape {
  userTurns: number;
  toolPairsPerTurn: number;
  thinkingPerTurn: number;
  textPerTurn: number;
  subagentsPerTurn: number;
  ticksPerSubagent: number;
  /** Subagents of the LAST turn never receive a terminal snapshot, so the
   *  transcript ends with this many still `running` (the shape that makes
   *  every settle pass do real work). */
  leaveLastTurnRunning: boolean;
}

/** ≈ the real profile's distribution: 15 turns × (173 tool pairs + 63
 *  thinking + 37 text + 8 subagents × 30 ticks) ≈ 7.7k events. */
export const REAL_SHAPE: BenchFixtureShape = {
  userTurns: 15,
  toolPairsPerTurn: 173,
  thinkingPerTurn: 63,
  textPerTurn: 37,
  subagentsPerTurn: 8,
  ticksPerSubagent: 30,
  leaveLastTurnRunning: true,
};

const SMALL_SHAPE: BenchFixtureShape = {
  userTurns: 3,
  toolPairsPerTurn: 20,
  thinkingPerTurn: 6,
  textPerTurn: 4,
  subagentsPerTurn: 2,
  ticksPerSubagent: 5,
  leaveLastTurnRunning: true,
};

/** Build the synthetic persisted event stream. Deterministic: same shape
 *  → byte-identical events, so the fold's output can be compared across
 *  runs / code changes. */
export function buildBenchTranscript(
  shape: BenchFixtureShape,
): TimedReplayPayload[] {
  const rows: TimedReplayPayload[] = [];
  const threadId = "bench-thread";
  let id = 1;
  let at = 1_700_000_000_000;
  const push = (event: ProviderRuntimeEvent) => {
    at += 7;
    rows.push({ event, createdAtMs: at, persistedId: id++ });
  };
  const item = (
    turnId: string,
    item: Extract<ProviderRuntimeEvent, { type: "item_completed" }>["item"],
  ) => push({ type: "item_completed", thread_id: threadId, turn_id: turnId, item });
  const snap = (subagent: SubagentSnapshot) =>
    push({ type: "subagent_updated", thread_id: threadId, subagent });

  for (let t = 0; t < shape.userTurns; t++) {
    const turnId = `turn-${t}`;
    push({
      type: "user_message",
      thread_id: threadId,
      text: `user turn ${t}: please do the thing`,
      client_nonce: `nonce-${t}`,
    });
    push({
      type: "session_state_changed",
      thread_id: threadId,
      status: { status: "running", active_turn: turnId },
    });

    // Spawn this turn's subagents up front (Claude keys the subagent id on
    // the spawning Agent tool_use_id, and settles it from that tool's
    // parent-scoped tool_result when the terminal snapshot goes missing).
    const spawnIds: string[] = [];
    for (let s = 0; s < shape.subagentsPerTurn; s++) {
      const useId = `agent-${t}-${s}`;
      spawnIds.push(useId);
      item(turnId, {
        kind: "tool_use",
        tool_use_id: useId,
        tool_name: "Agent",
        input: { description: `explore ${s}`, prompt: "look around" },
      });
      snap({
        subagent_id: useId,
        parent_item_id: useId,
        status: "running",
        name: `Explore ${s}`,
        agent_type: "Explore",
        description: `explore ${s}`,
      });
    }

    // Interleave tool pairs, thinking, text, and subagent ticks so the
    // subagent cards are NOT at the tail when most events land (the
    // realistic case: ticks arrive while the parent keeps calling tools).
    const cycles = Math.max(
      shape.toolPairsPerTurn,
      shape.thinkingPerTurn,
      shape.textPerTurn,
      shape.subagentsPerTurn * shape.ticksPerSubagent,
    );
    let toolN = 0;
    let thinkN = 0;
    let textN = 0;
    let tickN = 0;
    const totalTicks = shape.subagentsPerTurn * shape.ticksPerSubagent;
    for (let c = 0; c < cycles; c++) {
      if (thinkN < shape.thinkingPerTurn && c % 3 === 0) {
        item(turnId, {
          kind: "assistant_thinking",
          text: `thinking ${t}/${thinkN} about the next step in some detail`,
        });
        thinkN++;
      }
      if (toolN < shape.toolPairsPerTurn) {
        const useId = `tool-${t}-${toolN}`;
        item(turnId, {
          kind: "tool_use",
          tool_use_id: useId,
          tool_name: toolN % 2 === 0 ? "Read" : "Bash",
          input: { file_path: `/src/file-${toolN}.ts` },
        });
        item(turnId, {
          kind: "tool_result",
          tool_use_id: useId,
          content: `result ${toolN} `.repeat(20),
          is_error: false,
        });
        toolN++;
      }
      if (tickN < totalTicks && shape.subagentsPerTurn > 0) {
        const s = tickN % shape.subagentsPerTurn;
        snap({
          subagent_id: spawnIds[s],
          status: "running",
          activity: `reading file ${tickN}`,
          tool_use_count: Math.floor(tickN / shape.subagentsPerTurn),
        });
        tickN++;
      }
      if (textN < shape.textPerTurn && c % 5 === 0) {
        item(turnId, {
          kind: "assistant_text",
          text: `Progress note ${t}/${textN}: still working on it.`,
        });
        textN++;
      }
    }

    const last = t === shape.userTurns - 1;
    if (!(last && shape.leaveLastTurnRunning)) {
      for (const useId of spawnIds) {
        snap({
          subagent_id: useId,
          status: "completed",
          result_text: "done",
          duration_ms: 12_000,
        });
        // The suppressed spawn tool_result leaks through on some resumes.
        item(turnId, {
          kind: "tool_result",
          tool_use_id: useId,
          content: "agent finished",
          is_error: false,
        });
      }
      item(turnId, { kind: "assistant_text", text: `Turn ${t} summary.` });
      push({
        type: "turn_completed",
        thread_id: threadId,
        turn_id: turnId,
        status: { kind: "success" },
        usage: null,
      });
      push({
        type: "session_state_changed",
        thread_id: threadId,
        status: { status: "ready" },
      });
    }
  }
  return rows;
}

function histogram(rows: TimedReplayPayload[]): Record<string, number> {
  const out: Record<string, number> = {};
  for (const row of rows) {
    const key =
      row.event.type === "item_completed"
        ? `item_completed/${row.event.item.kind}`
        : row.event.type;
    out[key] = (out[key] ?? 0) + 1;
  }
  return out;
}

const BENCH = process.env.CODEMUX_BENCH === "1";

describe("cold hydrate replay (bench)", () => {
  it("keeps the incrementally-maintained transcript index exact through a full replay", () => {
    // Every append/replace in the reducer must route through the
    // index-carrying primitives. If one is missed the index is silently
    // stale (a settle pass would skip a running subagent, or a repeated
    // tool_use_id would mint a duplicate row). Compare the index the
    // replay carried along with one rebuilt from the final rows.
    const rows = buildBenchTranscript(SMALL_SHAPE);
    __resetReducerIdCounterForTests();
    const state = replayTimed(rows);
    const carried = transcriptIndex(state.messages);
    const rebuilt = transcriptIndex(state.messages.slice());
    expect(carried).not.toBe(rebuilt);
    expect(carried.workflowCount).toBe(rebuilt.workflowCount);
    expect(carried.permissionRequestCount).toBe(rebuilt.permissionRequestCount);
    expect([...carried.toolUseIds]).toEqual([...rebuilt.toolUseIds]);
    expect([...carried.runningSubagentKeys]).toEqual([
      ...rebuilt.runningSubagentKeys,
    ]);
    // The fixture's tool_use_ids all landed as rows.
    expect(carried.toolUseIds.size).toBe(
      rows.filter(
        (r) => r.event.type === "item_completed" && r.event.item.kind === "tool_use",
      ).length,
    );
  });

  it("folds the synthetic transcript deterministically", () => {
    const shape = BENCH ? REAL_SHAPE : SMALL_SHAPE;
    const rows = buildBenchTranscript(shape);
    __resetReducerIdCounterForTests();
    const a = replayTimed(rows);
    __resetReducerIdCounterForTests();
    const b = replayTimed(rows);
    expect(a).toEqual(b);
    expect(a.messages.length).toBeGreaterThan(0);
    expect(a.streaming).toBe(false);
    // The last turn's subagents were left running → hydrate must settle
    // them so no spinner survives a cold load.
    const lastCard = [...a.messages]
      .reverse()
      .find((m) => m.kind === "subagent_run");
    expect(lastCard?.kind).toBe("subagent_run");
    if (lastCard?.kind === "subagent_run") {
      for (const sub of lastCard.subagents) {
        expect(sub.status).toBe("interrupted");
        expect(sub.statusAssumed).toBe(true);
      }
    }
  });

  it.runIf(BENCH)("times replayTimed over the real-shaped fixture", () => {
    const rows = buildBenchTranscript(REAL_SHAPE);
    const iterations = 5;
    const samples: number[] = [];
    // Warm-up (JIT) — not counted.
    __resetReducerIdCounterForTests();
    replayTimed(rows);
    let state = replayTimed(rows);
    for (let i = 0; i < iterations; i++) {
      __resetReducerIdCounterForTests();
      const t0 = performance.now();
      state = replayTimed(rows);
      samples.push(performance.now() - t0);
    }
    samples.sort((x, y) => x - y);
    const median = samples[Math.floor(samples.length / 2)];
    const mean = samples.reduce((s, x) => s + x, 0) / samples.length;
    // eslint-disable-next-line no-console
    console.log(
      [
        `[hydrate-bench] events=${rows.length} items=${state.messages.length}`,
        `histogram=${JSON.stringify(histogram(rows))}`,
        `replayTimed ms: min=${samples[0].toFixed(1)} median=${median.toFixed(1)} mean=${mean.toFixed(1)} max=${samples[samples.length - 1].toFixed(1)}`,
      ].join("\n"),
    );
    expect(state.messages.length).toBeGreaterThan(0);
  });
});
