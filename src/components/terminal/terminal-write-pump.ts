/**
 * Throttled, ordered write pump for a single xterm Terminal.
 *
 * Every byte destined for the terminal — disk scrollback restore, the PTY
 * reattach replay, and steady live output — is funneled through one pump
 * instance. On a workspace/tab switch the freshly mounted pane reattaches the
 * PTY output channel, and the backend's `attach_pty_output` replays its
 * `pending_output` ring as up to OUTPUT_BUFFER_BYTE_LIMIT / PTY_BATCH_SIZE
 * (≈ 256 MiB / 32 KiB ≈ 8000) back-to-back 32 KiB chunks. Writing that whole
 * backlog to xterm in one synchronous run pegs the renderer's main thread
 * (Tauri IPC dispatch + xterm escape-sequence parsing) for hundreds of ms to
 * seconds — the "freeze on click, then text pours in" symptom. Restoring a
 * multi-thousand-line disk scrollback in a single `term.write` does the same.
 *
 * The pump drains up to `budgetBytes` per macrotask and then yields, so paint
 * and input interleave between batches: the pane is interactive immediately
 * and history fills in over a few frames instead of blocking the switch.
 *
 * Ordering: a single FIFO queue with one producer path per source and one
 * drain loop guarantees bytes hit xterm in enqueue order. That matters because
 * xterm's parser is stateful — historical bytes must be written before live
 * bytes or the alt-screen / cursor state across the boundary scrambles. Callers
 * enqueue scrollback first, then attach the live channel.
 *
 * Single-flight: `drain` guards on `pumpRunning`, so a kick during an active
 * drain is a no-op and the running loop picks up newly queued items on its next
 * iteration. `cancel()` stops the loop (checked between every write) and drops
 * the queue, so an unmount mid-drain can't write into a disposed terminal.
 *
 * This is the consumer-side throttle first written for the (now-dead)
 * persistent-cache path in commit 59081d8, extracted here so the live
 * `TerminalPane` path uses it and it is unit-testable in isolation.
 */

/** Default bytes written per macrotask before yielding. Kept modest so a
 *  single batch's parse cost stays well under a frame; the yield between
 *  batches is what keeps input/paint responsive during a large replay. */
export const DEFAULT_PUMP_BUDGET_BYTES = 256 * 1024;

/** Default slice size for `enqueueString`. A large scrollback string is cut
 *  into pieces of this many UTF-16 code units (never splitting a surrogate
 *  pair) so it shares the same throttled drain as live output. */
export const DEFAULT_PUMP_SLICE_CHARS = 256 * 1024;

/**
 * Default PTY producer flow-control watermarks (queued bytes).
 *
 * `enqueue`d PTY bytes that haven't yet been handed to xterm accumulate in the
 * pump's FIFO. Under a sustained flood — `yes`, `cat huge-file`, a verbose
 * build, a runaway agent — a fast producer outruns the throttled drain and the
 * queue would grow without bound, ballooning renderer memory. When the queued
 * byte total crosses HIGH the pump fires `onHighWatermark` (the live path asks
 * the backend to pause the PTY read loop, so the child blocks on `write()` once
 * the kernel PTY buffer fills — real backpressure to the producer). It fires
 * `onLowWatermark` once the queue drains back below LOW. The watermarks are
 * deliberately generous so ordinary bursts and the multi-MB reattach replay
 * (already spread out by the pump) don't trip backpressure — only genuine
 * floods do. Matches the constants from the prior (disabled) cache design.
 */
export const DEFAULT_FLOW_HIGH_WATERMARK_BYTES = 16 * 1024 * 1024; // 16 MiB
export const DEFAULT_FLOW_LOW_WATERMARK_BYTES = 4 * 1024 * 1024; // 4 MiB

export interface WritePumpOptions {
  budgetBytes?: number;
  sliceChars?: number;
  /** Macrotask yield between batches. Injectable so tests can drive the pump
   *  deterministically; defaults to `setTimeout(resolve, 0)`. */
  yieldToMacrotask?: () => Promise<void>;
  /** Queued-byte HIGH watermark. When the total bytes still waiting to be
   *  written first exceeds this, `onHighWatermark` fires once. Defaults to
   *  `DEFAULT_FLOW_HIGH_WATERMARK_BYTES`. */
  highWatermarkBytes?: number;
  /** Queued-byte LOW watermark. After a HIGH crossing, when the queue drains
   *  back below this, `onLowWatermark` fires once. Defaults to
   *  `DEFAULT_FLOW_LOW_WATERMARK_BYTES`. */
  lowWatermarkBytes?: number;
  /** Fired exactly once each time the queue crosses from below HIGH to above
   *  it (hysteresis: re-arms only after a subsequent drop below LOW). The live
   *  path pauses the PTY read loop here. Errors thrown are not caught — keep
   *  the callback non-throwing. */
  onHighWatermark?: () => void;
  /** Fired exactly once each time the queue drains below LOW after having been
   *  above HIGH. The live path resumes the PTY read loop here. */
  onLowWatermark?: () => void;
}

export interface TerminalWritePump {
  /** Enqueue raw bytes (a PTY output chunk) for the throttled drain. */
  enqueue(data: Uint8Array): void;
  /** Slice a large string into budget-sized pieces (surrogate-safe) and
   *  enqueue them in order. Used for the one-shot disk scrollback restore. */
  enqueueString(data: string): void;
  /** Stop draining and drop anything still queued. Idempotent. After this,
   *  further `enqueue`/`enqueueString` calls are ignored. Does NOT fire the
   *  watermark callbacks — the caller owns resume-on-teardown. */
  cancel(): void;
  /** True once `cancel()` has been called. */
  readonly cancelled: boolean;
  /** Number of items still waiting to be written. Test/debug helper. */
  readonly pending: number;
  /** Total byte length of items still waiting to be written (the value the
   *  watermark checks use). Test/debug helper. */
  readonly queuedBytes: number;
  /** True while the queue is above HIGH and hasn't yet drained below LOW —
   *  i.e. the pump currently believes flow control is engaged. Lets the caller
   *  decide whether to issue a resume on teardown. */
  readonly flowPaused: boolean;
}

export function createWritePump(
  write: (data: Uint8Array | string) => void,
  options: WritePumpOptions = {},
): TerminalWritePump {
  const budgetBytes = options.budgetBytes ?? DEFAULT_PUMP_BUDGET_BYTES;
  const sliceChars = options.sliceChars ?? DEFAULT_PUMP_SLICE_CHARS;
  const yieldToMacrotask =
    options.yieldToMacrotask ??
    (() => new Promise<void>((resolve) => setTimeout(resolve, 0)));
  const highWatermarkBytes =
    options.highWatermarkBytes ?? DEFAULT_FLOW_HIGH_WATERMARK_BYTES;
  const lowWatermarkBytes =
    options.lowWatermarkBytes ?? DEFAULT_FLOW_LOW_WATERMARK_BYTES;
  const onHighWatermark = options.onHighWatermark;
  const onLowWatermark = options.onLowWatermark;

  const queue: Array<Uint8Array | string> = [];
  let pumpRunning = false;
  let cancelled = false;
  // Running total of `queue` item byte lengths, kept in lockstep with the
  // queue (enqueue adds, drain subtracts) so the watermark check is O(1)
  // instead of summing the queue. String slices count their UTF-16 length —
  // consistent with how the budget accounting treats them.
  let queuedBytes = 0;
  // Hysteresis latch: true between a HIGH crossing and the subsequent drop
  // below LOW. Gates both callbacks so each fires exactly once per cycle and
  // ordinary bursts that hover near a watermark don't thrash pause/resume.
  let overHigh = false;

  // Called right after bytes are appended. Fires the HIGH callback at most
  // once per cycle (re-armed only after a LOW crossing clears `overHigh`).
  const checkHighWatermark = (): void => {
    if (overHigh) return;
    if (queuedBytes <= highWatermarkBytes) return;
    overHigh = true;
    onHighWatermark?.();
  };

  // Called after each write removes bytes from the queue. Fires the LOW
  // callback once the queue has drained back below LOW after a HIGH crossing.
  const checkLowWatermark = (): void => {
    if (!overHigh) return;
    if (queuedBytes >= lowWatermarkBytes) return;
    overHigh = false;
    onLowWatermark?.();
  };

  const drain = async (): Promise<void> => {
    if (pumpRunning) return;
    pumpRunning = true;
    try {
      while (queue.length > 0 && !cancelled) {
        let budget = budgetBytes;
        while (queue.length > 0 && budget > 0 && !cancelled) {
          const next = queue.shift()!;
          budget -= next.length;
          // Account for the chunk leaving the queue before writing, so the
          // watermark reflects only bytes still waiting on us.
          queuedBytes -= next.length;
          if (queuedBytes < 0) queuedBytes = 0;
          write(next);
          checkLowWatermark();
        }
        // Yield a macrotask so the browser can paint the rows just written
        // and service input before the next batch.
        await yieldToMacrotask();
      }
    } finally {
      pumpRunning = false;
    }
  };

  return {
    enqueue(data: Uint8Array): void {
      if (cancelled) return;
      queue.push(data);
      queuedBytes += data.length;
      checkHighWatermark();
      void drain();
    },
    enqueueString(data: string): void {
      if (cancelled || data.length === 0) return;
      let i = 0;
      while (i < data.length) {
        let end = Math.min(i + sliceChars, data.length);
        if (end < data.length) {
          const code = data.charCodeAt(end - 1);
          // Don't split a UTF-16 surrogate pair across two writes: if the
          // slice would end on a high surrogate, pull its low surrogate in.
          if (code >= 0xd800 && code <= 0xdbff) end += 1;
        }
        const slice = data.slice(i, end);
        queue.push(slice);
        queuedBytes += slice.length;
        i = end;
      }
      checkHighWatermark();
      void drain();
    },
    cancel(): void {
      cancelled = true;
      queue.length = 0;
      queuedBytes = 0;
      // Leave `overHigh` as-is: the caller inspects `flowPaused` on teardown to
      // decide whether to resume the backend. Firing onLowWatermark here would
      // race that explicit resume.
    },
    get cancelled(): boolean {
      return cancelled;
    },
    get pending(): number {
      return queue.length;
    },
    get queuedBytes(): number {
      return queuedBytes;
    },
    get flowPaused(): boolean {
      return overHigh;
    },
  };
}
