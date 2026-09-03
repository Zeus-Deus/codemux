import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import {
  NO_INTERACTION,
  abandonInteraction,
  beginInteraction,
  clearTraces,
  configureInteractionTrace,
  endSubMeasure,
  exportDiagnostics,
  getTraces,
  isInteractionTraceEnabled,
  mark,
  markOpenInteraction,
  nearestRankPercentile,
  parseRendererInfo,
  rendererDiagnostics,
  startSubMeasure,
  summarizeTraces,
} from "./interaction-trace";

/**
 * Faking `performance` alongside the timers makes `performance.now()` advance
 * with `vi.advanceTimersByTime`, so phase durations are exact instead of
 * whatever the host machine happened to do. Faked rAF fires on a 16 ms timer,
 * which is exactly the double-rAF the module arms for the paint mark.
 */
function useFakeClock(): void {
  vi.useFakeTimers({
    toFake: [
      "setTimeout",
      "clearTimeout",
      "requestAnimationFrame",
      "cancelAnimationFrame",
      "performance",
      "Date",
    ],
  });
}

/** Two frames, so the double-rAF resolves. */
const TWO_FRAMES_MS = 40;

/** The post-paint grace a painted trace waits out when its closing phase
 *  (`state-committed`) never arrives. */
const POST_PAINT_GRACE_MS = 3_000;

interface SwitchTimings {
  invoke?: number;
  delivery?: number;
  commit?: number;
  mount?: number;
}

/** Drive one complete workspace-switch trace through every phase. */
function runSwitch(target: string, timings: SwitchTimings = {}): void {
  const { invoke = 3, delivery = 20, commit = 9, mount = 50 } = timings;
  const id = beginInteraction("workspace-switch", { target });
  mark(id, "click");
  mark(id, "invoke-start");
  vi.advanceTimersByTime(invoke);
  mark(id, "invoke-returned");
  vi.advanceTimersByTime(delivery);
  markOpenInteraction("snapshot-received", { target });
  vi.advanceTimersByTime(commit);
  markOpenInteraction("state-committed", { target });
  vi.advanceTimersByTime(mount);
  markOpenInteraction("pane-mounted", { target });
  markOpenInteraction("pane-content-ready", { target, meta: { paneKind: 1 } });
  markOpenInteraction("pane-interactive", { target, meta: { paneKind: 1 } });
  vi.advanceTimersByTime(TWO_FRAMES_MS);
}

beforeEach(() => {
  useFakeClock();
  configureInteractionTrace({ enabled: true, console: false });
  clearTraces();
});

afterEach(() => {
  vi.useRealTimers();
  configureInteractionTrace({ enabled: true, console: false });
  clearTraces();
});

describe("interaction trace — lifecycle", () => {
  it("attributes each phase of a completed workspace switch", () => {
    runSwitch("ws-1", { invoke: 3, delivery: 20, commit: 9, mount: 50 });

    const traces = getTraces();
    expect(traces).toHaveLength(1);
    const trace = traces[0];
    expect(trace.kind).toBe("workspace-switch");
    expect(trace.abandoned).toBe(false);
    expect(trace.complete).toBe(true);
    expect(trace.spans).toMatchObject({
      invoke: 3,
      "state-event": 23,
      commit: 9,
      mount: 82,
    });
    // Paint lands on the second rAF, i.e. two 16 ms frames after the mount.
    expect(trace.spans.paint).toBeGreaterThan(0);
    expect(trace.totalMs).toBeGreaterThanOrEqual(82);
  });

  it("records every phase in order, first stamp winning per phase", () => {
    const id = beginInteraction("workspace-switch", { target: "ws-1" });
    mark(id, "click");
    vi.advanceTimersByTime(5);
    // A repeat of an already-stamped phase must not move the mark: under
    // backend churn the same phase is reached many times and the first
    // arrival is the one that bounds latency.
    mark(id, "click");
    mark(id, "invoke-start");
    vi.advanceTimersByTime(1);
    mark(id, "invoke-returned");
    markOpenInteraction("snapshot-received", { target: "ws-1" });
    markOpenInteraction("state-committed", { target: "ws-1" });
    markOpenInteraction("pane-mounted", { target: "ws-1" });
    markOpenInteraction("pane-content-ready", { target: "ws-1" });
    markOpenInteraction("pane-interactive", { target: "ws-1" });
    vi.advanceTimersByTime(TWO_FRAMES_MS);

    const trace = getTraces()[0];
    expect(trace.marks.map((m) => m.phase)).toEqual([
      "click",
      "invoke-start",
      "invoke-returned",
      "snapshot-received",
      "state-committed",
      "pane-mounted",
      "pane-content-ready",
      "pane-interactive",
      "painted",
    ]);
    expect(trace.marks[0].atMs).toBe(0);
  });

  it("omits spans whose phases never arrived", () => {
    const id = beginInteraction("workspace-switch", { target: "ws-1" });
    mark(id, "click");
    vi.advanceTimersByTime(4);
    // No invoke marks at all — a switch driven entirely by the backend.
    markOpenInteraction("snapshot-received", { target: "ws-1" });
    markOpenInteraction("state-committed", { target: "ws-1" });
    markOpenInteraction("pane-mounted", { target: "ws-1" });
    markOpenInteraction("pane-content-ready", { target: "ws-1" });
    markOpenInteraction("pane-interactive", { target: "ws-1" });
    vi.advanceTimersByTime(TWO_FRAMES_MS);

    const trace = getTraces()[0];
    expect(trace.spans.invoke).toBeUndefined();
    expect(trace.spans.delivery).toBeUndefined();
    expect(trace.spans.commit).toBeDefined();
  });

  it("abandons a trace that never reaches its final phase", () => {
    const id = beginInteraction("workspace-switch", { target: "ws-1" });
    mark(id, "click");
    mark(id, "invoke-start");
    // Pane never mounts (the target rendered a diff/editor tab instead).
    vi.advanceTimersByTime(10_000);

    const traces = getTraces();
    expect(traces).toHaveLength(1);
    expect(traces[0].abandoned).toBe(true);
    expect(traces[0].marks.map((m) => m.phase)).not.toContain("painted");
  });

  it("closes a trace immediately when the invoke rejects", () => {
    const id = beginInteraction("workspace-switch", { target: "ws-1" });
    mark(id, "click");
    abandonInteraction(id);

    expect(getTraces()).toHaveLength(1);
    expect(getTraces()[0].abandoned).toBe(true);
    // The abandon timeout must have been cleared, not left to fire twice.
    vi.advanceTimersByTime(20_000);
    expect(getTraces()).toHaveLength(1);
  });

  it("ignores marks for an id that already completed", () => {
    const id = beginInteraction("workspace-switch", { target: "ws-1" });
    mark(id, "click");
    abandonInteraction(id);
    expect(() => mark(id, "pane-mounted")).not.toThrow();
    expect(getTraces()).toHaveLength(1);
  });

  // The real order once selection paints optimistically: the pane mounts in the
  // click's own task, so the paint lands long before the backend answers.
  it("keeps a painted trace open for the backend marks that follow it", () => {
    const id = beginInteraction("workspace-switch", { target: "ws-2" });
    mark(id, "click");
    mark(id, "invoke-start");
    markOpenInteraction("pane-mounted", { target: "ws-2" });
    markOpenInteraction("pane-content-ready", { target: "ws-2" });
    markOpenInteraction("pane-interactive", { target: "ws-2" });
    vi.advanceTimersByTime(TWO_FRAMES_MS);

    // Painted, but the round-trip is still out — nothing may be closed yet.
    expect(getTraces()).toHaveLength(0);

    vi.advanceTimersByTime(30);
    mark(id, "invoke-returned");
    vi.advanceTimersByTime(20);
    markOpenInteraction("snapshot-received", { target: "ws-2" });
    vi.advanceTimersByTime(9);
    markOpenInteraction("state-committed", { target: "ws-2" });

    const traces = getTraces();
    expect(traces).toHaveLength(1);
    expect(traces[0].abandoned).toBe(false);
    // The decomposition the exit gate needs survived the early paint.
    expect(traces[0].marks.map((m) => m.phase)).toEqual([
      "click",
      "invoke-start",
      "pane-mounted",
      "pane-content-ready",
      "pane-interactive",
      "painted",
      "invoke-returned",
      "snapshot-received",
      "state-committed",
    ]);
    expect(traces[0].spans.invoke).toBe(70);
    expect(traces[0].spans["state-event"]).toBe(90);
    expect(traces[0].spans.commit).toBe(9);
    expect(Object.values(traces[0].spans).every((duration) => duration >= 0)).toBe(true);
  });

  it("closes a painted trace on the grace when the commit never arrives", () => {
    const id = beginInteraction("workspace-switch", { target: "ws-3" });
    mark(id, "click");
    markOpenInteraction("pane-mounted", { target: "ws-3" });
    vi.advanceTimersByTime(TWO_FRAMES_MS);
    expect(getTraces()).toHaveLength(0);

    vi.advanceTimersByTime(POST_PAINT_GRACE_MS);
    const traces = getTraces();
    expect(traces).toHaveLength(1);
    // It painted, so it is a real measurement — not an abandoned one.
    expect(traces[0].abandoned).toBe(false);
    expect(traces[0].complete).toBe(false);
    expect(traces[0].marks.map((m) => m.phase)).toContain("painted");

    // The grace must not leave the 10 s abandon timeout behind it.
    vi.advanceTimersByTime(20_000);
    expect(getTraces()).toHaveLength(1);
  });
});

describe("interaction trace — open-trace targeting", () => {
  it("does not let a snapshot for another workspace claim the stamp", () => {
    const id = beginInteraction("workspace-switch", { target: "ws-target" });
    mark(id, "click");
    mark(id, "invoke-start");
    vi.advanceTimersByTime(5);
    // A streaming emit for the workspace the user is leaving.
    markOpenInteraction("snapshot-received", { target: "ws-other" });
    vi.advanceTimersByTime(30);
    markOpenInteraction("snapshot-received", { target: "ws-target" });
    markOpenInteraction("state-committed", { target: "ws-target" });
    mark(id, "invoke-returned");
    markOpenInteraction("pane-mounted", { target: "ws-target" });
    markOpenInteraction("pane-content-ready", { target: "ws-target" });
    markOpenInteraction("pane-interactive", { target: "ws-target" });
    vi.advanceTimersByTime(TWO_FRAMES_MS);

    const trace = getTraces()[0];
    const snapshot = trace.marks.find((m) => m.phase === "snapshot-received");
    expect(snapshot?.atMs).toBe(35);
  });

  it("marks the newest open trace when several are in flight", () => {
    const first = beginInteraction("workspace-switch", { target: "ws-1" });
    mark(first, "click");
    const second = beginInteraction("workspace-switch", { target: "ws-2" });
    mark(second, "click");
    vi.advanceTimersByTime(7);
    // No target given: the most recent interaction is the live one.
    markOpenInteraction("pane-mounted");
    vi.advanceTimersByTime(TWO_FRAMES_MS + POST_PAINT_GRACE_MS);

    const traces = getTraces();
    expect(traces).toHaveLength(1);
    expect(traces[0].id).toBe(second);
  });

  it("does nothing when no trace is open", () => {
    expect(() => markOpenInteraction("snapshot-received")).not.toThrow();
    expect(getTraces()).toHaveLength(0);
  });
});

describe("interaction trace — sub-measures", () => {
  it("attributes a timed block to the open interaction", () => {
    const id = beginInteraction("workspace-switch", { target: "ws-1" });
    mark(id, "click");
    const started = startSubMeasure();
    vi.advanceTimersByTime(30);
    endSubMeasure("terminal-teardown", started);
    markOpenInteraction("pane-mounted", { target: "ws-1" });
    vi.advanceTimersByTime(TWO_FRAMES_MS + POST_PAINT_GRACE_MS);

    expect(getTraces()[0].subMeasures).toEqual([{ name: "terminal-teardown", ms: 30 }]);
  });

  it("drops a sub-measure taken with no interaction open", () => {
    const started = startSubMeasure();
    vi.advanceTimersByTime(30);
    expect(() => endSubMeasure("terminal-teardown", started)).not.toThrow();
    expect(getTraces()).toHaveLength(0);
  });

  it("files a block that started inside a just-closed trace on that trace", () => {
    // The tail-first chat open fires content-ready (closing the switch
    // trace) and then keeps working — backfill + final replay. Those
    // blocks began inside the interaction and belong to it.
    // Earlier tests may leave traces open under a since-discarded fake
    // clock (their abandon timers never fire). Toggling the gate drops
    // them, so this test's closed trace is the only candidate.
    configureInteractionTrace({ enabled: false, console: false });
    configureInteractionTrace({ enabled: true, console: false });
    clearTraces();
    runSwitch("ws-1");
    expect(getTraces()).toHaveLength(1);
    const started = startSubMeasure(); // right after the close
    vi.advanceTimersByTime(120);
    endSubMeasure("hydrate:backfill", started);
    expect(getTraces()[0].subMeasures).toEqual([{ name: "hydrate:backfill", ms: 120 }]);

    // But not a block that started long after the trace closed.
    vi.advanceTimersByTime(10_000);
    const late = startSubMeasure();
    vi.advanceTimersByTime(10);
    endSubMeasure("unrelated", late);
    expect(getTraces()[0].subMeasures).toHaveLength(1);
  });
});

describe("interaction trace — disabled gate", () => {
  it("makes every entry point a no-op", () => {
    configureInteractionTrace({ enabled: false, console: false });
    expect(isInteractionTraceEnabled()).toBe(false);

    const id = beginInteraction("workspace-switch", { target: "ws-1" });
    expect(id).toBe(NO_INTERACTION);
    mark(id, "click");
    markOpenInteraction("snapshot-received", { target: "ws-1" });
    markOpenInteraction("pane-mounted", { target: "ws-1" });
    expect(startSubMeasure()).toBeNull();
    endSubMeasure("terminal-teardown", null);
    abandonInteraction(id);
    vi.advanceTimersByTime(20_000);

    expect(getTraces()).toHaveLength(0);
  });

  it("drops in-flight traces when tracing is turned off mid-interaction", () => {
    const id = beginInteraction("workspace-switch", { target: "ws-1" });
    mark(id, "click");
    configureInteractionTrace({ enabled: false, console: false });
    vi.advanceTimersByTime(20_000);
    expect(getTraces()).toHaveLength(0);
  });
});

describe("interaction trace — ring buffer", () => {
  it("keeps only the most recent 100 completed traces", () => {
    for (let i = 0; i < 130; i += 1) {
      runSwitch(`ws-${i}`, { invoke: 1, delivery: 1, commit: 1, mount: 1 });
    }
    const traces = getTraces();
    expect(traces).toHaveLength(100);
    // The oldest 30 were evicted, so ids start at 31 (1-based counter).
    expect(traces[traces.length - 1].id - traces[0].id).toBe(99);
  });

  it("hands out a copy, so callers cannot mutate the buffer", () => {
    runSwitch("ws-1");
    const traces = getTraces();
    traces.length = 0;
    expect(getTraces()).toHaveLength(1);
  });
});

describe("interaction trace — aggregation", () => {
  it("computes nearest-rank percentiles", () => {
    const values = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
    expect(nearestRankPercentile(values, 50)).toBe(50);
    expect(nearestRankPercentile(values, 95)).toBe(100);
    expect(nearestRankPercentile(values, 100)).toBe(100);
    // Order must not matter.
    expect(nearestRankPercentile([3, 1, 2], 50)).toBe(2);
    // Single sample: every percentile is that sample.
    expect(nearestRankPercentile([42], 95)).toBe(42);
    expect(nearestRankPercentile([], 95)).toBe(0);
  });

  it("summarizes p50/p95/p99/max and failure rate per interaction kind", () => {
    for (const mount of [10, 20, 30, 40, 200]) {
      runSwitch("ws-1", { invoke: 1, delivery: 1, commit: 1, mount });
    }
    const [summary] = summarizeTraces();
    expect(summary.kind).toBe("workspace-switch");
    expect(summary.count).toBe(5);
    expect(summary.abandoned).toBe(0);
    expect(summary.spans.mount).toMatchObject({
      count: 5,
      p50: 33,
      p95: 203,
      p99: 203,
      max: 203,
    });
    expect(summary.failureRate).toBe(0);
    expect(summary.total.max).toBeGreaterThanOrEqual(200);
  });

  it("counts abandoned traces separately", () => {
    runSwitch("ws-1");
    const id = beginInteraction("workspace-switch", { target: "ws-2" });
    mark(id, "click");
    vi.advanceTimersByTime(10_000);

    const [summary] = summarizeTraces();
    expect(summary.count).toBe(2);
    expect(summary.abandoned).toBe(1);
    expect(summary.incomplete).toBe(1);
    expect(summary.failureRate).toBe(0.5);
    // Incomplete/abandoned samples are visible as counts but cannot skew the
    // release-gate percentiles.
    expect(summary.total.count).toBe(1);
  });

  it("never exports a negative span when snapshot and optimistic paint beat invoke return", () => {
    const id = beginInteraction("workspace-switch", { target: "ws-race" });
    mark(id, "click");
    mark(id, "invoke-start");
    markOpenInteraction("pane-mounted", { target: "ws-race" });
    markOpenInteraction("pane-content-ready", { target: "ws-race" });
    markOpenInteraction("pane-interactive", { target: "ws-race" });
    vi.advanceTimersByTime(TWO_FRAMES_MS);
    markOpenInteraction("snapshot-received", { target: "ws-race" });
    vi.advanceTimersByTime(2);
    markOpenInteraction("state-committed", { target: "ws-race" });
    vi.advanceTimersByTime(20);
    mark(id, "invoke-returned");

    const trace = getTraces()[0];
    expect(trace.complete).toBe(true);
    expect(trace.spans.delivery).toBeUndefined();
    expect(trace.spans["state-event"]).toBeGreaterThanOrEqual(0);
    expect(Object.values(trace.spans).every((duration) => duration >= 0)).toBe(true);
  });

  it("excludes a trace with a reversed causal pair from release percentiles", () => {
    const id = beginInteraction("workspace-switch", { target: "ws-reversed" });
    mark(id, "click");
    mark(id, "invoke-start");
    mark(id, "invoke-returned");
    // Deliberately impossible causal ordering: committing before receipt must
    // make the evidence incomplete even though every named phase arrives.
    markOpenInteraction("state-committed", { target: "ws-reversed" });
    vi.advanceTimersByTime(5);
    markOpenInteraction("snapshot-received", { target: "ws-reversed" });
    markOpenInteraction("pane-mounted", { target: "ws-reversed" });
    markOpenInteraction("pane-content-ready", { target: "ws-reversed" });
    markOpenInteraction("pane-interactive", { target: "ws-reversed" });
    vi.advanceTimersByTime(TWO_FRAMES_MS);

    const trace = getTraces()[0];
    expect(trace.marks).toHaveLength(9);
    expect(trace.spans.commit).toBeUndefined();
    expect(trace.complete).toBe(false);
    expect(summarizeTraces()[0]).toMatchObject({
      count: 1,
      incomplete: 1,
      total: { count: 0 },
    });
  });

  it("retains summary evidence for a full 500-switch sweep", () => {
    for (let index = 0; index < 500; index += 1) {
      runSwitch(`ws-${index}`, { mount: index % 7 });
    }

    expect(getTraces()).toHaveLength(100);
    expect(summarizeTraces()[0]).toMatchObject({ count: 500, incomplete: 0 });
    expect(exportDiagnostics().traceCount).toBe(500);
  });
});

describe("interaction trace — long task observation", () => {
  it("degrades to zero long tasks when neither entry type is supported", () => {
    // The test environment exposes a PerformanceObserver whose
    // `supportedEntryTypes` covers neither "longtask" nor
    // "long-animation-frame" — the same situation as a WebKitGTK build that
    // supports neither, so tracing must still complete normally.
    const supported = (PerformanceObserver as unknown as {
      supportedEntryTypes?: readonly string[];
    }).supportedEntryTypes;
    expect(supported).not.toContain("longtask");
    expect(supported).not.toContain("long-animation-frame");

    runSwitch("ws-1");

    expect(getTraces()[0].longTasks).toEqual([]);
    expect(exportDiagnostics().observedEntryTypes).toEqual([]);
  });

  it("does not construct an observer when supportedEntryTypes is missing", () => {
    const construct = vi.fn();
    class ObserverWithoutSupportList {
      constructor() {
        construct();
      }
      observe() {}
      disconnect() {}
    }
    vi.stubGlobal("PerformanceObserver", ObserverWithoutSupportList);
    try {
      runSwitch("ws-1");
      expect(construct).not.toHaveBeenCalled();
      expect(getTraces()).toHaveLength(1);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("survives an entry type that is advertised but not observable", () => {
    class ThrowingObserver {
      static supportedEntryTypes = ["longtask"];
      observe() {
        throw new Error("not observable in this context");
      }
      disconnect() {}
    }
    vi.stubGlobal("PerformanceObserver", ThrowingObserver);
    try {
      expect(() => runSwitch("ws-1")).not.toThrow();
      expect(getTraces()[0].longTasks).toEqual([]);
    } finally {
      vi.unstubAllGlobals();
    }
  });
});

describe("interaction trace — diagnostics export", () => {
  it("carries durations and counts but no workspace identifiers", () => {
    runSwitch("ws-super-secret-client-name");
    const diagnostics = exportDiagnostics();
    const serialized = JSON.stringify(diagnostics);

    expect(diagnostics.version).toBe(3);
    expect(diagnostics.traceCount).toBe(1);
    expect(diagnostics.summaries).toHaveLength(1);
    expect(diagnostics.traces[0].spans.mount).toBe(82);
    expect(serialized).not.toContain("ws-super-secret-client-name");
    expect(serialized).not.toContain("secret");
  });

  it("reports the gate state", () => {
    expect(exportDiagnostics().enabled).toBe(true);
    configureInteractionTrace({ enabled: false, console: false });
    expect(exportDiagnostics().enabled).toBe(false);
  });
});

describe("renderer diagnostics", () => {
  const WEBKITGTK_UA =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";
  const CHROME_UA =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

  it("parses the WebKit versions out of a WebKitGTK UA", () => {
    expect(parseRendererInfo(WEBKITGTK_UA, 2)).toEqual({
      userAgent: WEBKITGTK_UA,
      webkitVersion: "605.1.15",
      webkitReleaseVersion: "17.4",
      linuxWebKitGtk: true,
      devicePixelRatio: 2,
    });
  });

  it("does not claim WebKitGTK on a Chromium UA", () => {
    const info = parseRendererInfo(CHROME_UA, 1);
    expect(info.linuxWebKitGtk).toBe(false);
    expect(info.webkitVersion).toBe("537.36");
    expect(info.webkitReleaseVersion).toBeNull();
  });

  it("tolerates an engine that reports no WebKit token", () => {
    const info = parseRendererInfo("Some/1.0 Engine", null);
    expect(info.webkitVersion).toBeNull();
    expect(info.webkitReleaseVersion).toBeNull();
    expect(info.devicePixelRatio).toBeNull();
  });

  it("reports the terminal probe as unrun rather than running it", () => {
    // Read-only by design: asking for diagnostics must not create a throwaway
    // GL context on a machine that has not opened a terminal.
    expect(rendererDiagnostics().terminalWebgl).toBeNull();
  });

  it("stays free of paths and identifiers", () => {
    const serialized = JSON.stringify(exportDiagnostics().renderer);
    expect(serialized).not.toContain("/home/");
    expect(serialized).not.toContain("workspace");
  });
});
