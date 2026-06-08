import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createWritePump } from "./terminal-write-pump";

/**
 * Run every pending macrotask yield (default `setTimeout(0)`), including the
 * timers the drain loop reschedules for each subsequent batch, until the pump
 * has fully drained. `runAllTimersAsync` recurses through self-rescheduling
 * timers and flushes the chained microtasks between them.
 */
async function drainFully(): Promise<void> {
  await vi.runAllTimersAsync();
}

function bytes(...vals: number[]): Uint8Array {
  return new Uint8Array(vals);
}

describe("terminal-write-pump", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("delivers a rapid burst of chunks to the writer in enqueue order", async () => {
    const writes: Uint8Array[] = [];
    const pump = createWritePump((d) => writes.push(d as Uint8Array));

    for (let i = 0; i < 200; i++) pump.enqueue(bytes(i & 0xff));
    await drainFully();

    expect(writes.length).toBe(200);
    expect(writes.map((w) => w[0])).toEqual(
      Array.from({ length: 200 }, (_, i) => i & 0xff),
    );
  });

  it("preserves order across enqueueString then enqueue (history before live)", async () => {
    const writes: Array<string | Uint8Array> = [];
    const pump = createWritePump((d) => writes.push(d), { sliceChars: 4 });

    // "history" string sliced into pieces, then a "live" byte chunk.
    pump.enqueueString("HISTORYDATA"); // 11 chars -> slices of 4,4,3
    pump.enqueue(bytes(76, 73, 86, 69)); // "LIVE"
    await drainFully();

    const joined = writes
      .map((w) =>
        typeof w === "string" ? w : String.fromCharCode(...(w as Uint8Array)),
      )
      .join("");
    expect(joined).toBe("HISTORYDATALIVE");
    // The live chunk must be the LAST write — never interleaved into history.
    expect(typeof writes[writes.length - 1]).not.toBe("string");
  });

  it("writes at most one budget worth of bytes per batch, then yields", async () => {
    const writes: Uint8Array[] = [];
    // An injected yield gate lets us release exactly one batch at a time,
    // which is more precise than 0-delay fake timers for this assertion.
    let release: (() => void) | null = null;
    const gate = () =>
      new Promise<void>((resolve) => {
        release = resolve;
      });
    const flushMicrotasks = async () => {
      for (let i = 0; i < 4; i++) await Promise.resolve();
    };

    // budget = 4 bytes; each chunk is 4 bytes -> exactly one chunk per batch.
    const pump = createWritePump((d) => writes.push(d as Uint8Array), {
      budgetBytes: 4,
      yieldToMacrotask: gate,
    });

    pump.enqueue(bytes(1, 1, 1, 1));
    pump.enqueue(bytes(2, 2, 2, 2));
    pump.enqueue(bytes(3, 3, 3, 3));

    // The first batch runs synchronously on enqueue; the rest wait on the gate.
    expect(writes.length).toBe(1);
    expect(pump.pending).toBe(2);

    release!();
    await flushMicrotasks();
    expect(writes.length).toBe(2);

    release!();
    await flushMicrotasks();
    expect(writes.length).toBe(3);
  });

  it("enqueueString slices a large string into multiple writes", async () => {
    const writes: string[] = [];
    const pump = createWritePump((d) => writes.push(d as string), {
      sliceChars: 10,
    });

    const data = "x".repeat(95); // 95 / 10 -> 10 slices (9x10 + 1x5)
    pump.enqueueString(data);
    await drainFully();

    expect(writes.length).toBe(10);
    expect(writes.join("")).toBe(data); // lossless
    expect(writes.every((w) => typeof w === "string")).toBe(true);
  });

  it("never splits a UTF-16 surrogate pair across slices", async () => {
    const writes: string[] = [];
    const pump = createWritePump((d) => writes.push(d as string), {
      sliceChars: 3,
    });

    // "ab😀cd😀" — each emoji is a surrogate pair (2 code units). With a slice
    // size of 3, naive slicing would cut a pair; the pump must keep pairs whole.
    const data = "ab\u{1F600}cd\u{1F600}";
    pump.enqueueString(data);
    await drainFully();

    expect(writes.join("")).toBe(data); // lossless
    for (const w of writes) {
      // No slice may start with a low surrogate or end with a high surrogate.
      const first = w.charCodeAt(0);
      const last = w.charCodeAt(w.length - 1);
      expect(first >= 0xdc00 && first <= 0xdfff).toBe(false);
      expect(last >= 0xd800 && last <= 0xdbff).toBe(false);
    }
  });

  it("cancel() stops the drain mid-flight and drops the remaining queue", async () => {
    const writes: Uint8Array[] = [];
    const pump = createWritePump((d) => writes.push(d as Uint8Array), {
      budgetBytes: 1,
    });

    for (let i = 0; i < 50; i++) pump.enqueue(bytes(i));
    // First batch (1 byte) ran synchronously; 49 still queued.
    expect(writes.length).toBe(1);

    pump.cancel();
    expect(pump.cancelled).toBe(true);
    expect(pump.pending).toBe(0);

    await drainFully();
    // No further writes happened after cancel.
    expect(writes.length).toBe(1);
  });

  it("ignores enqueue/enqueueString after cancel()", async () => {
    const writes: Array<string | Uint8Array> = [];
    const pump = createWritePump((d) => writes.push(d));

    pump.cancel();
    pump.enqueue(bytes(1, 2, 3));
    pump.enqueueString("ignored");
    await drainFully();

    expect(writes.length).toBe(0);
    expect(pump.pending).toBe(0);
  });

  it("is single-flight: re-entrant enqueues during a drain do not corrupt order", async () => {
    const writes: number[] = [];
    const pump = createWritePump((d) => writes.push((d as Uint8Array)[0]), {
      budgetBytes: 1,
    });

    pump.enqueue(bytes(0));
    // Enqueue more while the first drain is parked on its timer yield.
    pump.enqueue(bytes(1));
    pump.enqueue(bytes(2));
    await drainFully();
    pump.enqueue(bytes(3)); // kicks a fresh drain after the previous finished
    await drainFully();

    expect(writes).toEqual([0, 1, 2, 3]);
  });

  it("empty enqueueString is a no-op", async () => {
    const writes: unknown[] = [];
    const pump = createWritePump((d) => writes.push(d));
    pump.enqueueString("");
    await drainFully();
    expect(writes.length).toBe(0);
  });

  // ── PTY producer back-pressure (queued-byte watermarks) ──
  //
  // Under fake timers the first enqueue drains one budget synchronously then
  // parks on its setTimeout(0); every subsequent synchronous enqueue piles up
  // (the pump is single-flight). That lets these tests build the queue past a
  // watermark deterministically without releasing the drain.

  it("fires onHighWatermark once when the queue exceeds HIGH, onLowWatermark once after it drains", async () => {
    const writes: Uint8Array[] = [];
    let highCalls = 0;
    let lowCalls = 0;
    const pump = createWritePump((d) => writes.push(d as Uint8Array), {
      highWatermarkBytes: 10,
      lowWatermarkBytes: 4,
      onHighWatermark: () => highCalls++,
      onLowWatermark: () => lowCalls++,
    });

    // 20 one-byte chunks. The first drains synchronously (queue→0); the rest
    // pile up, so queuedBytes climbs 1,2,…,19 and crosses HIGH (10) once.
    for (let i = 0; i < 20; i++) pump.enqueue(bytes(i & 0xff));

    expect(highCalls).toBe(1);
    expect(lowCalls).toBe(0);
    expect(pump.flowPaused).toBe(true);
    expect(pump.queuedBytes).toBe(19);

    await drainFully();

    expect(lowCalls).toBe(1);
    expect(highCalls).toBe(1); // never re-fired while staying high
    expect(pump.flowPaused).toBe(false);
    expect(pump.queuedBytes).toBe(0);
    expect(writes.length).toBe(20); // back-pressure never drops bytes
  });

  it("does not trip watermarks for an ordinary sub-HIGH burst", async () => {
    let highCalls = 0;
    let lowCalls = 0;
    const pump = createWritePump(() => {}, {
      highWatermarkBytes: 1000,
      lowWatermarkBytes: 100,
      onHighWatermark: () => highCalls++,
      onLowWatermark: () => lowCalls++,
    });

    for (let i = 0; i < 20; i++) pump.enqueue(bytes(1, 2, 3)); // 60 bytes total
    await drainFully();

    expect(highCalls).toBe(0);
    expect(lowCalls).toBe(0);
    expect(pump.flowPaused).toBe(false);
  });

  it("counts enqueueString bytes toward the watermark", async () => {
    let highCalls = 0;
    const pump = createWritePump(() => {}, {
      highWatermarkBytes: 10,
      lowWatermarkBytes: 4,
      onHighWatermark: () => highCalls++,
    });

    // First enqueueString drains synchronously; the second piles up behind it.
    pump.enqueueString("AAAAA"); // 5 chars
    pump.enqueueString("BBBBBB"); // 6 chars -> queuedBytes = 6
    expect(highCalls).toBe(0);
    pump.enqueue(bytes(1, 2, 3, 4, 5)); // +5 -> queuedBytes = 11 > 10
    expect(highCalls).toBe(1);
    expect(pump.queuedBytes).toBe(11);
    expect(pump.flowPaused).toBe(true);
  });

  it("re-arms the high watermark only after dropping below LOW (hysteresis)", async () => {
    let highCalls = 0;
    let lowCalls = 0;
    const pump = createWritePump(() => {}, {
      highWatermarkBytes: 10,
      lowWatermarkBytes: 4,
      onHighWatermark: () => highCalls++,
      onLowWatermark: () => lowCalls++,
    });

    for (let i = 0; i < 20; i++) pump.enqueue(bytes(i & 0xff));
    expect(highCalls).toBe(1);
    await drainFully();
    expect(lowCalls).toBe(1);

    // Second flood after a full drain must arm HIGH again.
    for (let i = 0; i < 20; i++) pump.enqueue(bytes(i & 0xff));
    expect(highCalls).toBe(2);
    await drainFully();
    expect(lowCalls).toBe(2);
  });

  it("cancel() preserves flowPaused and does not fire onLowWatermark", async () => {
    let lowCalls = 0;
    const pump = createWritePump(() => {}, {
      highWatermarkBytes: 10,
      lowWatermarkBytes: 4,
      onLowWatermark: () => lowCalls++,
    });

    for (let i = 0; i < 20; i++) pump.enqueue(bytes(i & 0xff));
    expect(pump.flowPaused).toBe(true);

    pump.cancel();

    // The caller (TerminalPane) inspects flowPaused on teardown to decide
    // whether to resume the backend — cancel must NOT silently clear it or
    // fire the low callback (which would race that explicit resume).
    expect(pump.flowPaused).toBe(true);
    expect(lowCalls).toBe(0);
    expect(pump.pending).toBe(0);
    expect(pump.queuedBytes).toBe(0);
  });

  it("works without watermark callbacks (queuedBytes still tracked)", async () => {
    const writes: Uint8Array[] = [];
    const pump = createWritePump((d) => writes.push(d as Uint8Array));

    for (let i = 0; i < 5; i++) pump.enqueue(bytes(i, i)); // 2 bytes each
    // First chunk drained synchronously (2 bytes), 4 chunks (8 bytes) queued.
    expect(pump.queuedBytes).toBe(8);
    await drainFully();
    expect(pump.queuedBytes).toBe(0);
    expect(writes.length).toBe(5);
  });
});
