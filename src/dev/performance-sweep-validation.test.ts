import { describe, expect, it } from "vitest";
import type {
  CompletedTrace,
  KindSummary,
  PerfDiagnostics,
} from "@/lib/perf/interaction-trace";
import {
  collectSensitiveSnapshotValues,
  traceHasNonnegativeTimings,
  validatePerformanceSweep,
  WORKSPACE_SWITCH_P95_BUDGET_MS,
  type PerformanceSweepValidationInput,
  type StressRuntimeStats,
} from "./performance-sweep-validation";

const BEFORE: StressRuntimeStats = {
  active: true,
  emitted: 10,
  configuredPerSec: 100,
  providerRuntimeCommands: 0,
};

const AFTER: StressRuntimeStats = {
  active: true,
  emitted: 42,
  configuredPerSec: 100,
  providerRuntimeCommands: 0,
};

function trace(id: number): CompletedTrace {
  return {
    id,
    kind: "workspace-switch",
    totalMs: 10 + id,
    abandoned: false,
    complete: true,
    marks: [
      { phase: "click", atMs: 0 },
      {
        phase: "pane-content-ready",
        atMs: 8,
        meta: { paneKind: 1 },
      },
    ],
    spans: { paint: 10 + id },
    subMeasures: [{ name: "terminal-teardown", ms: 1 }],
    longTasks: [
      {
        entryType: "longtask",
        startMs: -2,
        durationMs: 5,
      },
    ],
  };
}

function summary(count: number): KindSummary {
  return {
    kind: "workspace-switch",
    count,
    abandoned: 0,
    incomplete: 0,
    failureRate: 0,
    total: { count, p50: 12, p95: 14, p99: 15, max: 16 },
    spans: {},
    longTasksPerTrace: { count, p50: 0, p95: 1, p99: 1, max: 1 },
  };
}

function diagnostics(count: number, switchSummary = summary(count)): PerfDiagnostics {
  return {
    version: 3,
    enabled: true,
    observedEntryTypes: [],
    renderer: {
      userAgent: "unit-test",
      webkitVersion: null,
      webkitReleaseVersion: null,
      linuxWebKitGtk: false,
      devicePixelRatio: 1,
      terminalWebgl: null,
    },
    startup: [],
    traceCount: count,
    summaries: [switchSummary],
    traces: [],
  };
}

function validInput(count = 3): PerformanceSweepValidationInput {
  return {
    expectedTraceCount: count,
    expectedPaneTraceCounts: { terminal: count, agentChat: 0 },
    p95BudgetMs: WORKSPACE_SWITCH_P95_BUDGET_MS,
    traces: Array.from({ length: count }, (_, index) => trace(index + 1)),
    diagnostics: diagnostics(count),
    serializedDiagnostics: JSON.stringify({ safe: true, traceCount: count }),
    redactionNeedles: ["secret-workspace", "/home/private/project"],
    stressBefore: BEFORE,
    stressAfter: AFTER,
    activationFailures: 0,
  };
}

describe("performance sweep validation", () => {
  it("accepts exact complete traces, a live delta driver, redacted diagnostics, and metrics", () => {
    const result = validatePerformanceSweep(validInput());

    expect(result).toMatchObject({
      ok: true,
      traceCount: 3,
      completeTraceCount: 3,
      nonnegativeTraceCount: 3,
      terminalTraceCount: 3,
      agentChatTraceCount: 0,
      deltaEventsDuringSweep: 32,
      metrics: { p50: 12, p95: 14, p99: 15, max: 16, failureRate: 0 },
      checks: {
        sustainedDeltaDriver: true,
        noProviderRuntimeCommands: true,
        paneCadence: true,
        diagnosticsRedacted: true,
        summaryMetrics: true,
        latencyBudget: true,
      },
    });
  });

  it("rejects incomplete, duplicate, and negative trace evidence", () => {
    const input = validInput();
    input.traces = [
      trace(1),
      { ...trace(1), complete: false },
      { ...trace(3), spans: { paint: -1 } },
    ];

    const result = validatePerformanceSweep(input);

    expect(result.ok).toBe(false);
    expect(result.checks.exactTraceCount).toBe(false);
    expect(result.checks.completeTraces).toBe(false);
    expect(result.checks.nonnegativeTraces).toBe(false);
  });

  it("allows a negative long-task start offset but never a negative duration", () => {
    const valid = trace(1);
    expect(traceHasNonnegativeTimings(valid)).toBe(true);
    expect(
      traceHasNonnegativeTimings({
        ...valid,
        longTasks: [{ ...valid.longTasks[0], durationMs: -0.1 }],
      }),
    ).toBe(false);
  });

  it("requires the declared terminal and agent-chat readiness mix", () => {
    const input = validInput();
    input.expectedPaneTraceCounts = { terminal: 2, agentChat: 1 };

    const result = validatePerformanceSweep(input);

    expect(result.ok).toBe(false);
    expect(result.checks.paneCadence).toBe(false);
    expect(result.errors).toContain(
      "pane trace mix was 3 terminal / 0 agent chat, expected 2 / 1",
    );
  });

  it("rejects leaked snapshot data, an idle driver, and provider runtime commands", () => {
    const input = validInput();
    input.serializedDiagnostics = JSON.stringify({
      workspace_id: "secret-workspace",
      value: "/home/private/project",
    });
    input.stressAfter = {
      ...AFTER,
      active: false,
      providerRuntimeCommands: 1,
    };

    const result = validatePerformanceSweep(input);

    expect(result.ok).toBe(false);
    expect(result.checks.diagnosticsRedacted).toBe(false);
    expect(result.checks.sustainedDeltaDriver).toBe(false);
    expect(result.checks.noProviderRuntimeCommands).toBe(false);
    expect(result.errors.join(" ")).not.toContain("/home/private/project");
  });

  it("requires ordered p50/p95/p99/max metrics and a zero failure rate", () => {
    const input = validInput();
    input.diagnostics = diagnostics(3, {
      ...summary(3),
      failureRate: 0.01,
      total: { count: 3, p50: 15, p95: 10, p99: 14, max: 12 },
    });

    const result = validatePerformanceSweep(input);

    expect(result.ok).toBe(false);
    expect(result.checks.summaryMetrics).toBe(false);
    expect(result.errors).toEqual(
      expect.arrayContaining([
        "workspace-switch percentiles are not monotonic",
        "workspace-switch failure rate was 0.01, expected 0",
      ]),
    );
  });

  it("rejects a sweep whose p95 exceeds the warm-switch budget", () => {
    const input = validInput();
    input.diagnostics = diagnostics(3, {
      ...summary(3),
      total: { count: 3, p50: 100, p95: 201, p99: 210, max: 220 },
    });

    const result = validatePerformanceSweep(input);

    expect(result.ok).toBe(false);
    expect(result.checks.latencyBudget).toBe(false);
    expect(result.errors).toContain(
      "workspace-switch p95 was 201ms, budget is 200ms",
    );
  });
});

it("collects nested identifying values without inventing generic scalar data", () => {
  const snapshot = {
    active_workspace_id: "ws-private",
    workspaces: [
      {
        workspace_id: "ws-private",
        title: "Secret Project",
        cwd: "/home/private/project",
        count: 17,
        config: { theme: "dark" },
      },
    ],
  };

  expect(collectSensitiveSnapshotValues(snapshot)).toEqual(
    expect.arrayContaining([
      "ws-private",
      "Secret Project",
      "/home/private/project",
    ]),
  );
  expect(collectSensitiveSnapshotValues(snapshot)).not.toContain("dark");
});
