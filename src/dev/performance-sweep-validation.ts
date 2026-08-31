import type {
  CompletedTrace,
  PerfDiagnostics,
  PhaseStats,
} from "@/lib/perf/interaction-trace";

export const PERFORMANCE_SWEEP_COUNT = 500;
export const WORKSPACE_SWITCH_P95_BUDGET_MS = 200;

export interface StressRuntimeStats {
  active: boolean;
  emitted: number;
  configuredPerSec: number;
  providerRuntimeCommands: number;
}

export interface PerformanceSweepMetrics {
  p50: number;
  p95: number;
  p99: number;
  max: number;
  failureRate: number;
}

export interface PerformanceSweepValidationInput {
  expectedTraceCount: number;
  expectedPaneTraceCounts: {
    terminal: number;
    agentChat: number;
  };
  p95BudgetMs: number;
  traces: readonly CompletedTrace[];
  diagnostics: PerfDiagnostics;
  serializedDiagnostics: string;
  redactionNeedles: readonly string[];
  stressBefore: StressRuntimeStats | null;
  stressAfter: StressRuntimeStats | null;
  activationFailures: number;
}

export interface PerformanceSweepValidationResult {
  ok: boolean;
  errors: string[];
  expectedTraceCount: number;
  traceCount: number;
  completeTraceCount: number;
  nonnegativeTraceCount: number;
  terminalTraceCount: number;
  agentChatTraceCount: number;
  deltaEventsDuringSweep: number;
  metrics: PerformanceSweepMetrics;
  checks: {
    exactTraceCount: boolean;
    completeTraces: boolean;
    nonnegativeTraces: boolean;
    paneCadence: boolean;
    sustainedDeltaDriver: boolean;
    noProviderRuntimeCommands: boolean;
    diagnosticsRedacted: boolean;
    summaryMetrics: boolean;
    latencyBudget: boolean;
  };
}

const FORBIDDEN_DIAGNOSTIC_KEYS = [
  "active_workspace_id",
  "workspace_id",
  "surface_id",
  "pane_id",
  "session_id",
  "thread_id",
  "browser_id",
  "tab_id",
  "cwd",
  "worktree_path",
  "project_root",
  "remote_cwd",
  "git_branch",
] as const;

const SENSITIVE_SNAPSHOT_VALUE_KEYS = new Set<string>([
  ...FORBIDDEN_DIAGNOSTIC_KEYS,
  "active_surface_id",
  "active_pane_id",
  "active_tab_id",
  "host_id",
  "pr_url",
  "title",
  "label",
]);

function isNonnegativeFinite(value: number): boolean {
  return Number.isFinite(value) && value >= 0;
}

function tracePaneKindCode(trace: CompletedTrace): number | null {
  const ready = trace.marks.find((mark) => mark.phase === "pane-content-ready");
  const paneKind = ready?.meta?.paneKind;
  return typeof paneKind === "number" ? paneKind : null;
}

/**
 * A long-task start offset may legitimately be negative when the task began
 * just before the click. Its duration, like every other measured duration,
 * must remain nonnegative.
 */
export function traceHasNonnegativeTimings(trace: CompletedTrace): boolean {
  return (
    isNonnegativeFinite(trace.totalMs) &&
    trace.marks.every((mark) => isNonnegativeFinite(mark.atMs)) &&
    Object.values(trace.spans).every(isNonnegativeFinite) &&
    trace.subMeasures.every((measure) => isNonnegativeFinite(measure.ms)) &&
    trace.longTasks.every((task) => isNonnegativeFinite(task.durationMs))
  );
}

/**
 * Extract exact sensitive values from a snapshot without retaining their
 * field names in the acceptance result. The validator only reports how many
 * leaked; it never repeats a path, title, or identifier into the visible UI.
 */
export function collectSensitiveSnapshotValues(snapshot: unknown): string[] {
  const values = new Set<string>();
  const visited = new WeakSet<object>();

  const visit = (value: unknown): void => {
    if (value === null || typeof value !== "object") return;
    if (visited.has(value)) return;
    visited.add(value);

    if (Array.isArray(value)) {
      for (const child of value) visit(child);
      return;
    }

    for (const [key, child] of Object.entries(value)) {
      if (
        SENSITIVE_SNAPSHOT_VALUE_KEYS.has(key) &&
        typeof child === "string" &&
        child.trim().length >= 4
      ) {
        values.add(child);
      }
      visit(child);
    }
  };

  visit(snapshot);
  return [...values];
}

function validateStats(
  stats: PhaseStats | undefined,
  expectedCount: number,
  errors: string[],
): stats is PhaseStats {
  if (!stats) {
    errors.push("workspace-switch summary metrics are missing");
    return false;
  }
  if (stats.count !== expectedCount) {
    errors.push(
      `workspace-switch metric sample count was ${stats.count}, expected ${expectedCount}`,
    );
  }
  const values = [stats.p50, stats.p95, stats.p99, stats.max];
  if (!values.every(isNonnegativeFinite)) {
    errors.push("workspace-switch summary contains an invalid percentile or maximum");
    return false;
  }
  if (!(stats.p50 <= stats.p95 && stats.p95 <= stats.p99 && stats.p99 <= stats.max)) {
    errors.push("workspace-switch percentiles are not monotonic");
    return false;
  }
  return stats.count === expectedCount;
}

export function validatePerformanceSweep(
  input: PerformanceSweepValidationInput,
): PerformanceSweepValidationResult {
  const errors: string[] = [];
  const traceCount = input.traces.length;
  const completeTraceCount = input.traces.filter(
    (trace) =>
      trace.kind === "workspace-switch" && trace.complete && !trace.abandoned,
  ).length;
  const nonnegativeTraceCount = input.traces.filter(
    traceHasNonnegativeTimings,
  ).length;
  const terminalTraceCount = input.traces.filter(
    (trace) => tracePaneKindCode(trace) === 1,
  ).length;
  const agentChatTraceCount = input.traces.filter(
    (trace) => tracePaneKindCode(trace) === 3,
  ).length;
  const uniqueTraceCount = new Set(input.traces.map((trace) => trace.id)).size;

  const exactTraceCount =
    traceCount === input.expectedTraceCount &&
    uniqueTraceCount === input.expectedTraceCount &&
    input.diagnostics.traceCount === input.expectedTraceCount;
  if (!exactTraceCount) {
    errors.push(
      `trace evidence was ${traceCount} collected / ${uniqueTraceCount} unique / ${input.diagnostics.traceCount} exported, expected ${input.expectedTraceCount}`,
    );
  }

  const completeTraces = completeTraceCount === input.expectedTraceCount;
  if (!completeTraces) {
    errors.push(
      `${completeTraceCount} of ${input.expectedTraceCount} workspace-switch traces were complete`,
    );
  }

  const nonnegativeTraces =
    nonnegativeTraceCount === input.expectedTraceCount;
  if (!nonnegativeTraces) {
    errors.push(
      `${nonnegativeTraceCount} of ${input.expectedTraceCount} traces had only nonnegative durations`,
    );
  }

  const paneCadence =
    terminalTraceCount === input.expectedPaneTraceCounts.terminal &&
    agentChatTraceCount === input.expectedPaneTraceCounts.agentChat &&
    terminalTraceCount + agentChatTraceCount === input.expectedTraceCount;
  if (!paneCadence) {
    errors.push(
      `pane trace mix was ${terminalTraceCount} terminal / ${agentChatTraceCount} agent chat, expected ${input.expectedPaneTraceCounts.terminal} / ${input.expectedPaneTraceCounts.agentChat}`,
    );
  }

  if (input.activationFailures !== 0) {
    errors.push(`${input.activationFailures} workspace activations rejected`);
  }

  const before = input.stressBefore;
  const after = input.stressAfter;
  const deltaEventsDuringSweep =
    before && after ? Math.max(0, after.emitted - before.emitted) : 0;
  const sustainedDeltaDriver = Boolean(
    before &&
      after &&
      before.active &&
      after.active &&
      before.configuredPerSec > 0 &&
      after.configuredPerSec === before.configuredPerSec &&
      after.emitted > before.emitted,
  );
  if (!sustainedDeltaDriver) {
    errors.push("the sustained delta driver was not active for the whole sweep");
  }

  const noProviderRuntimeCommands = Boolean(
    before &&
      after &&
      before.providerRuntimeCommands === 0 &&
      after.providerRuntimeCommands === 0,
  );
  if (!noProviderRuntimeCommands) {
    errors.push("provider runtime commands ran during app open or workspace switching");
  }

  const forbiddenKeys = FORBIDDEN_DIAGNOSTIC_KEYS.filter((key) =>
    input.serializedDiagnostics.includes(`"${key}":`),
  );
  const leakedValueCount = input.redactionNeedles.filter((needle) =>
    input.serializedDiagnostics.includes(JSON.stringify(needle)),
  ).length;
  const diagnosticsRedacted =
    input.serializedDiagnostics.length > 0 &&
    forbiddenKeys.length === 0 &&
    leakedValueCount === 0;
  if (forbiddenKeys.length > 0) {
    errors.push(
      `diagnostics retained forbidden identifying fields: ${forbiddenKeys.join(", ")}`,
    );
  }
  if (leakedValueCount > 0) {
    errors.push(`diagnostics retained ${leakedValueCount} sensitive snapshot value(s)`);
  }
  if (input.serializedDiagnostics.length === 0) {
    errors.push("diagnostics export was empty");
  }

  const summary = input.diagnostics.summaries.find(
    (candidate) => candidate.kind === "workspace-switch",
  );
  let summaryMetrics = false;
  let latencyBudget = false;
  const validLatencyBudget =
    Number.isFinite(input.p95BudgetMs) && input.p95BudgetMs > 0;
  if (!validLatencyBudget) {
    errors.push("workspace-switch p95 latency budget is invalid");
  }
  if (!summary) {
    errors.push("workspace-switch diagnostics summary is missing");
  } else {
    const statsValid = validateStats(
      summary.total,
      input.expectedTraceCount,
      errors,
    );
    const failureRateValid =
      Number.isFinite(summary.failureRate) && summary.failureRate === 0;
    if (!failureRateValid) {
      errors.push(`workspace-switch failure rate was ${summary.failureRate}, expected 0`);
    }
    if (
      summary.count !== input.expectedTraceCount ||
      summary.incomplete !== 0 ||
      summary.abandoned !== 0
    ) {
      errors.push(
        `workspace-switch summary counts do not describe ${input.expectedTraceCount} successful traces`,
      );
    }
    summaryMetrics =
      statsValid &&
      failureRateValid &&
      summary.count === input.expectedTraceCount &&
      summary.incomplete === 0 &&
      summary.abandoned === 0;
    latencyBudget =
      validLatencyBudget &&
      Number.isFinite(summary.total.p95) &&
      summary.total.p95 <= input.p95BudgetMs;
    if (!latencyBudget && validLatencyBudget) {
      errors.push(
        `workspace-switch p95 was ${summary.total.p95}ms, budget is ${input.p95BudgetMs}ms`,
      );
    }
  }

  const metrics: PerformanceSweepMetrics = {
    p50: summary?.total.p50 ?? 0,
    p95: summary?.total.p95 ?? 0,
    p99: summary?.total.p99 ?? 0,
    max: summary?.total.max ?? 0,
    failureRate: summary?.failureRate ?? 1,
  };

  const checks = {
    exactTraceCount,
    completeTraces,
    nonnegativeTraces,
    paneCadence,
    sustainedDeltaDriver,
    noProviderRuntimeCommands,
    diagnosticsRedacted,
    summaryMetrics,
    latencyBudget,
  };

  return {
    ok:
      errors.length === 0 &&
      Object.values(checks).every(Boolean) &&
      input.activationFailures === 0,
    errors,
    expectedTraceCount: input.expectedTraceCount,
    traceCount,
    completeTraceCount,
    nonnegativeTraceCount,
    terminalTraceCount,
    agentChatTraceCount,
    deltaEventsDuringSweep,
    metrics,
    checks,
  };
}
