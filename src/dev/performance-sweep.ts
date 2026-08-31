/**
 * Dev-only, real-UI acceptance sweep for workspace switching.
 *
 * This module is lazy-loaded by `main.tsx` only for
 * `?fixture=large&perfSweep=500`. It deliberately drives the production
 * activation function and only observes traces completed by the mounted UI;
 * it never calls the trace-marking APIs itself.
 */
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import {
  clearTraces,
  configureInteractionTrace,
  exportDiagnostics,
  getTraces,
  type CompletedTrace,
} from "@/lib/perf/interaction-trace";
import { collectPerformanceDiagnostics } from "@/lib/perf/performance-diagnostics";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { STRESS_THREAD_PREFIX } from "./stress-fixture";
import type {
  AppStateSnapshot,
  PaneNodeSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";
import {
  PERFORMANCE_SWEEP_COUNT,
  WORKSPACE_SWITCH_P95_BUDGET_MS,
  collectSensitiveSnapshotValues,
  validatePerformanceSweep,
  type PerformanceSweepValidationResult,
  type StressRuntimeStats,
} from "./performance-sweep-validation";

const STATUS_ID = "codemux-performance-sweep";
const APP_STATE_TIMEOUT_MS = 30_000;
const TRACE_TIMEOUT_MS = 15_000;
const POLL_INTERVAL_MS = 8;
const PROGRESS_INTERVAL = 25;
const CHAT_SWITCH_INTERVAL = 5;
const EXPECTED_CHAT_TRACES = Math.floor(
  PERFORMANCE_SWEEP_COUNT / CHAT_SWITCH_INTERVAL,
);
const EXPECTED_TERMINAL_TRACES = PERFORMANCE_SWEEP_COUNT - EXPECTED_CHAT_TRACES;

type PublicSweepState =
  | {
      status: "running";
      completed: number;
      expected: number;
    }
  | {
      status: "passed" | "failed";
      completed: number;
      expected: number;
      durationMs: number;
      result: PerformanceSweepValidationResult;
    }
  | {
      status: "error";
      completed: number;
      expected: number;
    };

function statusElement(): HTMLElement {
  const existing = document.getElementById(STATUS_ID);
  if (existing) return existing;

  const element = document.createElement("section");
  element.id = STATUS_ID;
  // A named region is retained by `codemux browser snapshot` (unlike static
  // status text, which its compact accessibility-tree view prunes).
  element.setAttribute("role", "region");
  element.setAttribute("aria-live", "polite");
  element.setAttribute("aria-atomic", "true");
  element.style.cssText = [
    "position:fixed",
    "top:12px",
    "right:12px",
    "z-index:2147483647",
    "max-width:min(560px,calc(100vw - 24px))",
    "padding:10px 12px",
    "border:1px solid rgba(245,158,11,.7)",
    "border-radius:8px",
    "background:rgba(9,9,11,.96)",
    "color:#f4f4f5",
    "box-shadow:0 10px 30px rgba(0,0,0,.35)",
    "font:12px/1.45 ui-monospace,SFMono-Regular,Menlo,monospace",
    "white-space:pre-wrap",
    "pointer-events:none",
  ].join(";");
  document.body.appendChild(element);
  return element;
}

function exposeState(state: PublicSweepState, text: string): void {
  const element = statusElement();
  element.dataset.status = state.status;
  element.dataset.completed = String(state.completed);
  element.dataset.expected = String(state.expected);
  element.dataset.result = JSON.stringify(state);
  element.setAttribute("aria-label", text.split("\n").join(" · "));
  element.textContent = text;
  (
    window as unknown as {
      __codemuxPerformanceSweep: PublicSweepState;
    }
  ).__codemuxPerformanceSweep = state;
}

function runningStatus(completed: number): void {
  exposeState(
    {
      status: "running",
      completed,
      expected: PERFORMANCE_SWEEP_COUNT,
    },
    [
      "Codemux performance sweep: RUNNING",
      `${completed}/${PERFORMANCE_SWEEP_COUNT} real workspace switches complete`,
      `${EXPECTED_TERMINAL_TRACES} terminal · ${EXPECTED_CHAT_TRACES} generated 15 MB chat · sustained deltas`,
      "provider runtime guard enabled",
    ].join("\n"),
  );
}

function finalStatus(
  result: PerformanceSweepValidationResult,
  durationMs: number,
): void {
  const status = result.ok ? "passed" : "failed";
  const metrics = result.metrics;
  const lines = [
    `Codemux performance sweep: ${result.ok ? "PASS" : "FAIL"}`,
    `${result.completeTraceCount}/${result.expectedTraceCount} complete · ${result.nonnegativeTraceCount}/${result.expectedTraceCount} nonnegative`,
    `${result.terminalTraceCount} terminal · ${result.agentChatTraceCount} generated chat traces`,
    `p50 ${metrics.p50.toFixed(2)} ms · p95 ${metrics.p95.toFixed(2)} ms · p99 ${metrics.p99.toFixed(2)} ms · max ${metrics.max.toFixed(2)} ms`,
    `failure rate ${(metrics.failureRate * 100).toFixed(2)}% · sustained deltas +${result.deltaEventsDuringSweep}`,
    `diagnostics ${result.checks.diagnosticsRedacted ? "redacted" : "FAILED redaction"} · provider runtime commands ${result.checks.noProviderRuntimeCommands ? "0" : "DETECTED"}`,
    `duration ${(durationMs / 1_000).toFixed(1)} s`,
  ];
  if (!result.ok) {
    lines.push(...result.errors.map((error) => `• ${error}`));
  }
  exposeState(
    {
      status,
      completed: result.traceCount,
      expected: result.expectedTraceCount,
      durationMs,
      result,
    },
    lines.join("\n"),
  );
}

function errorStatus(completed: number): void {
  exposeState(
    {
      status: "error",
      completed,
      expected: PERFORMANCE_SWEEP_COUNT,
    },
    [
      "Codemux performance sweep: ERROR",
      `${completed}/${PERFORMANCE_SWEEP_COUNT} switches completed before the harness stopped`,
      "See the developer console for the non-redacted harness error.",
    ].join("\n"),
  );
}

async function waitForValue<T>(
  read: () => T | null,
  timeoutMs: number,
  description: string,
): Promise<T> {
  const startedAt = performance.now();
  return new Promise<T>((resolve, reject) => {
    const poll = () => {
      try {
        const value = read();
        if (value !== null) {
          resolve(value);
          return;
        }
        if (performance.now() - startedAt >= timeoutMs) {
          reject(new Error(`Timed out waiting for ${description}`));
          return;
        }
        window.setTimeout(poll, POLL_INTERVAL_MS);
      } catch (error) {
        reject(error);
      }
    };
    poll();
  });
}

function readStressRuntimeStats(): StressRuntimeStats | null {
  const reader = (
    window as unknown as {
      __codemuxStressStats?: () => unknown;
    }
  ).__codemuxStressStats;
  if (typeof reader !== "function") return null;
  const value = reader() as Partial<StressRuntimeStats> | null;
  if (
    !value ||
    typeof value.active !== "boolean" ||
    !Number.isFinite(value.emitted) ||
    !Number.isFinite(value.configuredPerSec) ||
    !Number.isFinite(value.providerRuntimeCommands)
  ) {
    return null;
  }
  return {
    active: value.active,
    emitted: value.emitted as number,
    configuredPerSec: value.configuredPerSec as number,
    providerRuntimeCommands: value.providerRuntimeCommands as number,
  };
}

function findPane(node: PaneNodeSnapshot, paneId: string): PaneNodeSnapshot | null {
  if (node.pane_id === paneId) return node;
  if (node.kind !== "split") return null;
  for (const child of node.children) {
    const found = findPane(child, paneId);
    if (found) return found;
  }
  return null;
}

function activePane(workspace: WorkspaceSnapshot): PaneNodeSnapshot | null {
  const surface = workspace.surfaces.find(
    (candidate) => candidate.surface_id === workspace.active_surface_id,
  );
  if (!surface) return null;
  return findPane(surface.root, surface.active_pane_id);
}

function hasActiveTerminal(workspace: WorkspaceSnapshot): boolean {
  return activePane(workspace)?.kind === "terminal";
}

function terminalWorkspaceIds(snapshot: AppStateSnapshot): string[] {
  return snapshot.workspaces
    .filter(hasActiveTerminal)
    .map((workspace) => workspace.workspace_id);
}

function generatedAgentChatWorkspaceIds(snapshot: AppStateSnapshot): string[] {
  return snapshot.workspaces
    .filter((workspace) => {
      const pane = activePane(workspace);
      return (
        workspace.workspace_id.startsWith("ws-stress-") &&
        pane?.kind === "agent_chat" &&
        pane.thread_id?.startsWith(STRESS_THREAD_PREFIX)
      );
    })
    .slice(0, 2)
    .map((workspace) => workspace.workspace_id);
}

function nextWorkspaceId(ids: readonly string[], cursor: number): {
  id: string;
  cursor: number;
} {
  const current = selectActiveWorkspaceId(useAppStore.getState());
  for (let attempt = 0; attempt < ids.length; attempt += 1) {
    const nextCursor = (cursor + attempt) % ids.length;
    const id = ids[nextCursor];
    if (id !== current) {
      return { id, cursor: (nextCursor + 1) % ids.length };
    }
  }
  throw new Error("The large fixture did not provide two terminal workspaces");
}

async function waitForCompletedTrace(expectedTraceCount: number): Promise<CompletedTrace> {
  return waitForValue(() => {
    const traceCount = exportDiagnostics().traceCount;
    if (traceCount < expectedTraceCount) return null;
    if (traceCount > expectedTraceCount) {
      throw new Error(
        `Unexpected concurrent workspace trace: observed ${traceCount}, expected ${expectedTraceCount}`,
      );
    }
    const traces = getTraces();
    const trace = traces[traces.length - 1];
    if (!trace) return null;
    return trace;
  }, TRACE_TIMEOUT_MS, `workspace trace ${expectedTraceCount}`);
}

function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

async function runPerformanceSweep(): Promise<PerformanceSweepValidationResult> {
  const params = new URLSearchParams(location.search);
  if (
    params.get("fixture") !== "large" ||
    params.get("perfSweep") !== String(PERFORMANCE_SWEEP_COUNT)
  ) {
    throw new Error("Performance sweep requires ?fixture=large&perfSweep=500");
  }

  const startedAt = performance.now();
  let completed = 0;
  runningStatus(completed);

  const snapshot = await waitForValue(
    () => useAppStore.getState().appState,
    APP_STATE_TIMEOUT_MS,
    "the mounted app state",
  );
  const ids = terminalWorkspaceIds(snapshot);
  if (ids.length < 2) {
    throw new Error("The large fixture did not provide two terminal workspaces");
  }
  const chatIds = generatedAgentChatWorkspaceIds(snapshot);
  if (chatIds.length !== 2) {
    throw new Error("The large fixture did not provide two generated chat workspaces");
  }

  const stressBefore = await waitForValue(() => {
    const stats = readStressRuntimeStats();
    return stats?.active && stats.emitted > 0 ? stats : null;
  }, APP_STATE_TIMEOUT_MS, "the sustained delta driver");

  configureInteractionTrace({ enabled: true, console: false });
  clearTraces();

  const traces: CompletedTrace[] = [];
  let activationFailures = 0;
  let terminalCursor = 0;
  let chatCursor = 0;

  for (let index = 0; index < PERFORMANCE_SWEEP_COUNT; index += 1) {
    if (exportDiagnostics().traceCount !== index) {
      throw new Error(`Trace count changed outside sweep at switch ${index + 1}`);
    }
    const isChatSwitch = (index + 1) % CHAT_SWITCH_INTERVAL === 0;
    let workspaceId: string;
    if (isChatSwitch) {
      workspaceId = chatIds[chatCursor % chatIds.length];
      chatCursor += 1;
    } else {
      const next = nextWorkspaceId(ids, terminalCursor);
      workspaceId = next.id;
      terminalCursor = next.cursor;
    }
    try {
      await activateWorkspaceInteraction(workspaceId);
    } catch (error) {
      activationFailures += 1;
      console.error("[codemux::perf-sweep] workspace activation rejected", error);
    }
    traces.push(await waitForCompletedTrace(index + 1));
    completed = index + 1;

    if (
      completed % PROGRESS_INTERVAL === 0 ||
      completed === PERFORMANCE_SWEEP_COUNT
    ) {
      runningStatus(completed);
      // Paint progress before beginning the next measured interaction, so the
      // status overlay's own DOM work cannot land inside that trace.
      await nextFrame();
    }
  }

  const stressAfter = readStressRuntimeStats();
  const report = await collectPerformanceDiagnostics();
  const result = validatePerformanceSweep({
    expectedTraceCount: PERFORMANCE_SWEEP_COUNT,
    expectedPaneTraceCounts: {
      terminal: EXPECTED_TERMINAL_TRACES,
      agentChat: EXPECTED_CHAT_TRACES,
    },
    p95BudgetMs: WORKSPACE_SWITCH_P95_BUDGET_MS,
    traces,
    diagnostics: report.renderer,
    serializedDiagnostics: JSON.stringify(report),
    redactionNeedles: collectSensitiveSnapshotValues(snapshot),
    stressBefore,
    stressAfter,
    activationFailures,
  });
  finalStatus(result, performance.now() - startedAt);
  return result;
}

let activeSweep: Promise<PerformanceSweepValidationResult> | null = null;

/** Idempotent across React StrictMode and Vite hot updates. Reloading the URL
 * starts a fresh sweep with cleared trace evidence. */
export function startPerformanceSweep(): Promise<PerformanceSweepValidationResult> {
  if (activeSweep) return activeSweep;
  activeSweep = runPerformanceSweep().catch((error) => {
    const completed = exportDiagnostics().traceCount;
    errorStatus(completed);
    console.error("[codemux::perf-sweep] acceptance harness failed", error);
    throw error;
  });
  return activeSweep;
}
