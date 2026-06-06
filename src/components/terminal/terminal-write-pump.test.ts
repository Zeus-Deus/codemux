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
});
