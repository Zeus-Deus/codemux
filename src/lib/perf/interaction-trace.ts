/**
 * Interaction tracing — Phase 0 of `docs/plans/gui-responsiveness.md`.
 *
 * One trace per user interaction carries an id from the click through every
 * hand-off (IPC, snapshot delivery, Zustand commit, React mount, paint) so a
 * slow workspace switch is attributable to a named phase instead of a guess.
 *
 * Constraints this module is written to:
 *   - No React state and no store writes. Marking must never cause a render,
 *     otherwise the harness changes the thing it measures.
 *   - Off by default in production: a single boolean check short-circuits
 *     every entry point, so a disabled build pays one predictable branch.
 *   - Privacy-safe export: durations, phase names and counts only. Workspace
 *     ids are held in memory purely to match a mark to its trace and are
 *     never written into a trace record, a log line or the diagnostics blob.
 */

import { getCachedWebglProbe } from "@/components/terminal/webgl-renderer-probe";
import { isLinuxWebKitGtk } from "@/lib/webkit";

/** Interaction kinds. Grows as later phases instrument more paths. */
export type InteractionKind = "workspace-switch";

export type InteractionPhase =
  | "click"
  | "invoke-start"
  | "invoke-returned"
  | "snapshot-received"
  | "state-committed"
  | "pane-mounted"
  | "painted";

/** Mark metadata is numeric/boolean only so nothing identifying can reach the
 *  diagnostics export by accident. */
export type TraceMeta = Record<string, number | boolean>;

export interface TraceMark {
  phase: InteractionPhase;
  /** Offset from the start of the interaction, ms. */
  atMs: number;
  meta?: TraceMeta;
}

export interface SubMeasure {
  name: string;
  ms: number;
}

export interface LongTaskRecord {
  entryType: string;
  /** Offset from the start of the interaction, ms. May be negative if the
   *  task straddles the click. */
  startMs: number;
  durationMs: number;
}

export interface CompletedTrace {
  id: number;
  kind: InteractionKind;
  totalMs: number;
  /** True when the trace hit the abandon timeout without reaching its final
   *  phase (e.g. the target pane never mounted). */
  abandoned: boolean;
  marks: TraceMark[];
  /** Consecutive phase deltas, keyed by span label. Missing phases drop the
   *  span rather than reporting a bogus zero. */
  spans: Record<string, number>;
  subMeasures: SubMeasure[];
  longTasks: LongTaskRecord[];
}

export interface PhaseStats {
  count: number;
  p50: number;
  p95: number;
  max: number;
}

export interface KindSummary {
  kind: InteractionKind;
  count: number;
  abandoned: number;
  total: PhaseStats;
  spans: Record<string, PhaseStats>;
  longTasksPerTrace: PhaseStats;
}

interface SpanDef {
  label: string;
  from: InteractionPhase;
  to: InteractionPhase;
}

/** Non-overlapping decomposition: the spans sum to `total` minus the
 *  click→invoke-start gap, so one slow number always names one owner. */
const SPAN_DEFS: Record<InteractionKind, SpanDef[]> = {
  "workspace-switch": [
    { label: "invoke", from: "invoke-start", to: "invoke-returned" },
    { label: "delivery", from: "invoke-returned", to: "snapshot-received" },
    { label: "commit", from: "snapshot-received", to: "state-committed" },
    { label: "mount", from: "state-committed", to: "pane-mounted" },
    { label: "paint", from: "pane-mounted", to: "painted" },
  ],
};

/** Reaching this phase arms the double-rAF that stamps `painted`. */
const FINAL_PHASE: Record<InteractionKind, InteractionPhase> = {
  "workspace-switch": "pane-mounted",
};

/** The phase that closes a painted trace. Since selection paints optimistically,
 *  `painted` now happens in the click's own frame budget — well before the
 *  backend round-trip — so closing on paint alone would throw away exactly the
 *  spans (`invoke`, `delivery`, `commit`) the exit gate is about. */
const CLOSING_PHASE: Record<InteractionKind, InteractionPhase> = {
  "workspace-switch": "state-committed",
};

const RING_CAPACITY = 100;
const ABANDON_TIMEOUT_MS = 10_000;
/** How long a painted trace waits for its closing phase. Long enough for a slow
 *  backend round-trip to be attributed, short enough that the trace is not
 *  sitting open absorbing unrelated long-task entries. */
const POST_PAINT_GRACE_MS = 3_000;
/** How far back a late long-task entry may be attributed to an already
 *  completed trace. */
const LONGTASK_LOOKBACK_MS = 1_000;
const STORAGE_KEY = "codemux:perf-trace";

interface OpenTrace {
  id: number;
  kind: InteractionKind;
  /** In-memory only — never exported, never logged. */
  target: string | null;
  startedAt: number;
  marks: TraceMark[];
  subMeasures: SubMeasure[];
  longTasks: LongTaskRecord[];
  timeoutHandle: ReturnType<typeof setTimeout> | null;
  /** Set once the double-rAF is armed, so a repeated final phase can't arm a
   *  second one. */
  finalizing: boolean;
  /** Runs out the post-paint grace when the closing phase never arrives. */
  graceHandle: ReturnType<typeof setTimeout> | null;
}

const openTraces = new Map<number, OpenTrace>();
const ring: CompletedTrace[] = [];
/** Parallel to `ring`: absolute end timestamps, kept out of the exported
 *  record because a wall-clock-ish origin is not needed downstream. */
const ringEndedAt: number[] = [];
const ringStartedAt: number[] = [];
let nextId = 0;

function now(): number {
  return typeof performance !== "undefined" && typeof performance.now === "function"
    ? performance.now()
    : Date.now();
}

function roundMs(ms: number): number {
  return Math.round(ms * 100) / 100;
}

// ── Gate ────────────────────────────────────────────────────────────────

function readStorageFlag(): boolean {
  try {
    return typeof localStorage !== "undefined" && localStorage.getItem(STORAGE_KEY) === "1";
  } catch {
    // Private-mode / sandboxed webview: absent storage just means "off".
    return false;
  }
}

function detectEnabled(): boolean {
  if (readStorageFlag()) return true;
  return import.meta.env.DEV === true;
}

let traceEnabled = detectEnabled();
let consoleEnabled = true;

export function isInteractionTraceEnabled(): boolean {
  return traceEnabled;
}

/** Re-read the gate (after flipping the localStorage key at runtime) and/or
 *  force it. Tests use this to exercise the disabled no-op path. */
export function configureInteractionTrace(options: {
  enabled?: boolean;
  console?: boolean;
} = {}): void {
  traceEnabled = options.enabled ?? detectEnabled();
  if (options.console !== undefined) consoleEnabled = options.console;
  if (!traceEnabled) {
    for (const trace of openTraces.values()) {
      if (trace.timeoutHandle) clearTimeout(trace.timeoutHandle);
      if (trace.graceHandle) clearTimeout(trace.graceHandle);
    }
    openTraces.clear();
    stopObservers();
  }
}

// ── Long task / long animation frame observation ────────────────────────

const OBSERVED_ENTRY_TYPES = ["longtask", "long-animation-frame"] as const;
let observers: PerformanceObserver[] | null = null;

/** WebKitGTK may advertise neither entry type; `supportedEntryTypes` itself
 *  is absent on older engines, hence the two guards. */
function observableEntryTypes(): string[] {
  if (typeof PerformanceObserver === "undefined") return [];
  const supported = (PerformanceObserver as { supportedEntryTypes?: readonly string[] })
    .supportedEntryTypes;
  if (!Array.isArray(supported)) return [];
  return OBSERVED_ENTRY_TYPES.filter((type) => supported.includes(type));
}

function startObservers(): void {
  if (observers) return;
  observers = [];
  for (const type of observableEntryTypes()) {
    try {
      const observer = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          attributeLongEntry(type, entry.startTime, entry.duration);
        }
      });
      observer.observe({ type, buffered: false });
      observers.push(observer);
    } catch {
      // Advertised in `supportedEntryTypes` but not observable in this
      // context — treat as unavailable rather than failing the interaction.
    }
  }
}

function stopObservers(): void {
  if (!observers) return;
  for (const observer of observers) {
    try {
      observer.disconnect();
    } catch {
      /* already torn down */
    }
  }
  observers = null;
}

function attributeLongEntry(entryType: string, startTime: number, duration: number): void {
  const endTime = startTime + duration;
  let attributed = false;
  for (const trace of openTraces.values()) {
    if (endTime < trace.startedAt) continue;
    trace.longTasks.push({
      entryType,
      startMs: roundMs(startTime - trace.startedAt),
      durationMs: roundMs(duration),
    });
    attributed = true;
  }
  if (attributed) return;
  // Entries are delivered at the end of the offending task, which can land
  // just after the double-rAF closed the trace they belong to.
  const cutoff = now() - LONGTASK_LOOKBACK_MS;
  for (let i = ring.length - 1; i >= 0; i -= 1) {
    if (ringEndedAt[i] < cutoff) break;
    if (startTime < ringEndedAt[i] && endTime > ringStartedAt[i]) {
      ring[i].longTasks.push({
        entryType,
        startMs: roundMs(startTime - ringStartedAt[i]),
        durationMs: roundMs(duration),
      });
      return;
    }
  }
}

// ── User Timing mirror (so devtools traces line up) ──────────────────────

function userTimingName(trace: OpenTrace, phase: string): string {
  return `codemux/${trace.kind}#${trace.id}/${phase}`;
}

function emitUserTimingMark(trace: OpenTrace, phase: string): void {
  if (typeof performance === "undefined" || typeof performance.mark !== "function") return;
  try {
    performance.mark(userTimingName(trace, phase));
  } catch {
    /* User Timing unavailable or buffer full */
  }
}

function emitUserTimingMeasures(trace: OpenTrace, spans: Record<string, number>): void {
  if (typeof performance === "undefined" || typeof performance.measure !== "function") return;
  for (const def of SPAN_DEFS[trace.kind]) {
    if (spans[def.label] === undefined) continue;
    try {
      performance.measure(
        `codemux/${trace.kind}#${trace.id}/${def.label}`,
        userTimingName(trace, def.from),
        userTimingName(trace, def.to),
      );
    } catch {
      /* one of the marks was never stamped */
    }
  }
}

// ── Trace lifecycle ──────────────────────────────────────────────────────

/** Sentinel returned when tracing is off; every API call with it is a no-op. */
export const NO_INTERACTION = 0;

export function beginInteraction(
  kind: InteractionKind,
  options: { target?: string; meta?: TraceMeta } = {},
): number {
  if (!traceEnabled) return NO_INTERACTION;
  nextId += 1;
  const trace: OpenTrace = {
    id: nextId,
    kind,
    target: options.target ?? null,
    startedAt: now(),
    marks: [],
    subMeasures: [],
    longTasks: [],
    timeoutHandle: null,
    finalizing: false,
    graceHandle: null,
  };
  openTraces.set(trace.id, trace);
  startObservers();
  trace.timeoutHandle = setTimeout(() => completeTrace(trace, true), ABANDON_TIMEOUT_MS);
  if (options.meta) recordMark(trace, "click", options.meta);
  return trace.id;
}

export function mark(
  interactionId: number,
  phase: InteractionPhase,
  meta?: TraceMeta,
): void {
  if (!traceEnabled || interactionId === NO_INTERACTION) return;
  const trace = openTraces.get(interactionId);
  if (!trace) return;
  recordMark(trace, phase, meta);
}

/**
 * Mark the newest open trace without holding its id — used by the snapshot
 * listener and the pane mount, neither of which sees the click. `target`
 * scopes the mark to the interaction that asked for that workspace, so an
 * unrelated streaming emit can't steal the `snapshot-received` stamp.
 */
export function markOpenInteraction(
  phase: InteractionPhase,
  options: { kind?: InteractionKind; target?: string; meta?: TraceMeta } = {},
): void {
  if (!traceEnabled || openTraces.size === 0) return;
  const trace = findOpenTrace(options);
  if (!trace) return;
  recordMark(trace, phase, options.meta);
}

function findOpenTrace(options: { kind?: InteractionKind; target?: string }): OpenTrace | null {
  let found: OpenTrace | null = null;
  // Map iteration is insertion-ordered; the last match is the newest trace.
  for (const trace of openTraces.values()) {
    if (options.kind && trace.kind !== options.kind) continue;
    if (options.target !== undefined && trace.target !== null && trace.target !== options.target) {
      continue;
    }
    found = trace;
  }
  return found;
}

function hasMark(trace: OpenTrace, phase: InteractionPhase): boolean {
  for (const existing of trace.marks) {
    if (existing.phase === phase) return true;
  }
  return false;
}

function recordMark(trace: OpenTrace, phase: InteractionPhase, meta?: TraceMeta): void {
  // First stamp wins: under sustained backend churn the same phase can be
  // reached repeatedly, and the first arrival is the one that bounds latency.
  if (hasMark(trace, phase)) return;
  trace.marks.push({ phase, atMs: roundMs(now() - trace.startedAt), meta });
  emitUserTimingMark(trace, phase);
  if (!trace.finalizing && phase === FINAL_PHASE[trace.kind]) {
    trace.finalizing = true;
    doubleRaf(() => {
      if (!openTraces.has(trace.id)) return;
      recordMark(trace, "painted");
    });
    return;
  }
  // The trace closes on the later of "the user saw it" and "the backend
  // agreed" — in that order for a normal switch, but a slow round-trip
  // reverses them and both orders have to produce the same decomposition.
  if (phase === "painted" || phase === CLOSING_PHASE[trace.kind]) {
    settleTrace(trace);
  }
}

/** Close a trace once it has both painted and reached its closing phase; while
 *  only one of the two has happened, hold it open for the grace period so the
 *  other can still attach. */
function settleTrace(trace: OpenTrace): void {
  if (!hasMark(trace, "painted")) return;
  if (hasMark(trace, CLOSING_PHASE[trace.kind])) {
    completeTrace(trace, false);
    return;
  }
  trace.graceHandle ??= setTimeout(() => completeTrace(trace, false), POST_PAINT_GRACE_MS);
}

function doubleRaf(fn: () => void): void {
  const raf =
    typeof requestAnimationFrame === "function"
      ? requestAnimationFrame
      : (cb: FrameRequestCallback) => setTimeout(() => cb(now()), 0) as unknown as number;
  raf(() => {
    raf(() => fn());
  });
}

/** Drop an interaction that can never complete (the IPC rejected). */
export function abandonInteraction(interactionId: number): void {
  if (!traceEnabled || interactionId === NO_INTERACTION) return;
  const trace = openTraces.get(interactionId);
  if (trace) completeTrace(trace, true);
}

function computeSpans(trace: OpenTrace): Record<string, number> {
  const at = new Map<InteractionPhase, number>();
  for (const m of trace.marks) at.set(m.phase, m.atMs);
  const spans: Record<string, number> = {};
  for (const def of SPAN_DEFS[trace.kind]) {
    const from = at.get(def.from);
    const to = at.get(def.to);
    if (from === undefined || to === undefined) continue;
    spans[def.label] = roundMs(to - from);
  }
  return spans;
}

function completeTrace(trace: OpenTrace, abandoned: boolean): void {
  if (!openTraces.delete(trace.id)) return;
  if (trace.timeoutHandle) {
    clearTimeout(trace.timeoutHandle);
    trace.timeoutHandle = null;
  }
  if (trace.graceHandle) {
    clearTimeout(trace.graceHandle);
    trace.graceHandle = null;
  }
  const endedAt = now();
  const spans = computeSpans(trace);
  const completed: CompletedTrace = {
    id: trace.id,
    kind: trace.kind,
    totalMs: roundMs(endedAt - trace.startedAt),
    abandoned,
    marks: trace.marks,
    spans,
    subMeasures: trace.subMeasures,
    longTasks: trace.longTasks,
  };
  ring.push(completed);
  ringEndedAt.push(endedAt);
  ringStartedAt.push(trace.startedAt);
  while (ring.length > RING_CAPACITY) {
    ring.shift();
    ringEndedAt.shift();
    ringStartedAt.shift();
  }
  emitUserTimingMeasures(trace, spans);
  if (openTraces.size === 0) stopObservers();
  if (consoleEnabled) logSummary(completed);
}

function logSummary(trace: CompletedTrace): void {
  const parts = [`total=${Math.round(trace.totalMs)}ms`];
  for (const def of SPAN_DEFS[trace.kind]) {
    const value = trace.spans[def.label];
    if (value === undefined) continue;
    parts.push(`${def.label}=${Math.round(value)}ms`);
  }
  const bySubMeasure = new Map<string, number>();
  for (const sub of trace.subMeasures) {
    bySubMeasure.set(sub.name, (bySubMeasure.get(sub.name) ?? 0) + sub.ms);
  }
  for (const [name, ms] of bySubMeasure) parts.push(`${name}=${Math.round(ms)}ms`);
  parts.push(`longtasks=${trace.longTasks.length}`);
  if (trace.abandoned) parts.push("abandoned=1");
  // eslint-disable-next-line no-console
  console.info(`[codemux::perf] ${trace.kind} ${parts.join(" ")}`);
}

// ── Sub-measures (terminal teardown, and later phases' hot blocks) ───────

/** Returns a start stamp, or null when tracing is off. */
export function startSubMeasure(): number | null {
  return traceEnabled ? now() : null;
}

/** Attribute a timed block to the innermost open interaction. Dropped when no
 *  interaction is open — the point is attribution, not a second log channel. */
export function endSubMeasure(
  name: string,
  startedAt: number | null,
  options: { kind?: InteractionKind; target?: string } = {},
): void {
  if (!traceEnabled || startedAt === null) return;
  const trace = findOpenTrace(options);
  if (!trace) return;
  trace.subMeasures.push({ name, ms: roundMs(now() - startedAt) });
}

// ── Read-out ─────────────────────────────────────────────────────────────

export function getTraces(): CompletedTrace[] {
  return ring.slice();
}

export function clearTraces(): void {
  ring.length = 0;
  ringEndedAt.length = 0;
  ringStartedAt.length = 0;
}

/** Nearest-rank percentile over an unsorted sample. */
export function nearestRankPercentile(values: number[], percentile: number): number {
  if (values.length === 0) return 0;
  const sorted = values.slice().sort((a, b) => a - b);
  const rank = Math.ceil((percentile / 100) * sorted.length);
  return sorted[Math.min(sorted.length - 1, Math.max(0, rank - 1))];
}

function statsOf(values: number[]): PhaseStats {
  return {
    count: values.length,
    p50: roundMs(nearestRankPercentile(values, 50)),
    p95: roundMs(nearestRankPercentile(values, 95)),
    max: values.length === 0 ? 0 : roundMs(Math.max(...values)),
  };
}

export function summarizeTraces(traces: CompletedTrace[] = ring): KindSummary[] {
  const byKind = new Map<InteractionKind, CompletedTrace[]>();
  for (const trace of traces) {
    const bucket = byKind.get(trace.kind);
    if (bucket) bucket.push(trace);
    else byKind.set(trace.kind, [trace]);
  }
  const summaries: KindSummary[] = [];
  for (const [kind, bucket] of byKind) {
    const spans: Record<string, PhaseStats> = {};
    for (const def of SPAN_DEFS[kind]) {
      const values = bucket
        .map((t) => t.spans[def.label])
        .filter((v): v is number => v !== undefined);
      if (values.length > 0) spans[def.label] = statsOf(values);
    }
    summaries.push({
      kind,
      count: bucket.length,
      abandoned: bucket.filter((t) => t.abandoned).length,
      total: statsOf(bucket.map((t) => t.totalMs)),
      spans,
      longTasksPerTrace: statsOf(bucket.map((t) => t.longTasks.length)),
    });
  }
  return summaries;
}

/**
 * Which engine the renderer is actually running on, and what the terminal
 * decided about GPU acceleration.
 *
 * Two of the remaining latency outliers in this plan are engine-shaped rather
 * than code-shaped (WebKitGTK's compositing, a software GL stack), so a trace
 * that arrives without them is unattributable. Everything here is engine or
 * driver identity — no paths, hostnames, workspace ids or payload contents.
 */
export interface RendererDiagnostics {
  userAgent: string;
  /** `AppleWebKit/<version>` from the UA, or null on non-WebKit engines. */
  webkitVersion: string | null;
  /** WebKit's own release label (`Version/17.4`), which WebKitGTK reports
   *  alongside the fixed 605.1.15 AppleWebKit token. Null when absent. */
  webkitReleaseVersion: string | null;
  /** True when the UA identifies the Linux WebKitGTK webview — the app
   *  webview on Linux, and the engine with the known input-lag history. */
  linuxWebKitGtk: boolean;
  devicePixelRatio: number | null;
  /** The terminal's WebGL verdict for this session, or null when no terminal
   *  pane has mounted yet (the probe runs once, on first use). */
  terminalWebgl: {
    use: boolean;
    reason: string;
    /** GL renderer/adapter string, when the engine reports one honestly. */
    glRenderer: string | null;
  } | null;
}

/** Pure — exported so the UA parsing is testable without a browser. */
export function parseRendererInfo(
  userAgent: string,
  devicePixelRatio: number | null,
): Omit<RendererDiagnostics, "terminalWebgl"> {
  return {
    userAgent,
    webkitVersion: /AppleWebKit\/([\d.]+)/.exec(userAgent)?.[1] ?? null,
    webkitReleaseVersion: /(?:^|\s)Version\/([\d.]+)/.exec(userAgent)?.[1] ?? null,
    linuxWebKitGtk: isLinuxWebKitGtk(userAgent),
    devicePixelRatio,
  };
}

export function rendererDiagnostics(): RendererDiagnostics {
  const probe = getCachedWebglProbe();
  return {
    ...parseRendererInfo(
      typeof navigator === "undefined" ? "" : navigator.userAgent,
      typeof window === "undefined" ? null : window.devicePixelRatio,
    ),
    terminalWebgl: probe
      ? { use: probe.use, reason: probe.reason, glRenderer: probe.renderer }
      : null,
  };
}

export interface PerfDiagnostics {
  version: 2;
  enabled: boolean;
  observedEntryTypes: string[];
  renderer: RendererDiagnostics;
  traceCount: number;
  summaries: KindSummary[];
  traces: CompletedTrace[];
}

/**
 * Everything a bug report needs and nothing it doesn't: phase names, durations
 * and counts. No workspace ids, paths, titles or payload contents ever enter a
 * trace record, so this blob is safe to paste into an issue.
 */
export function exportDiagnostics(): PerfDiagnostics {
  return {
    version: 2,
    enabled: traceEnabled,
    observedEntryTypes: observableEntryTypes(),
    renderer: rendererDiagnostics(),
    traceCount: ring.length,
    summaries: summarizeTraces(),
    traces: getTraces(),
  };
}

// Expose the read-out on the window so a running build can be inspected from
// devtools without a rebuild. Attached only when the gate is already on.
if (traceEnabled && typeof window !== "undefined") {
  (window as unknown as { codemuxPerf?: unknown }).codemuxPerf = {
    getTraces,
    clearTraces,
    summarize: summarizeTraces,
    renderer: rendererDiagnostics,
    export: exportDiagnostics,
  };
}
