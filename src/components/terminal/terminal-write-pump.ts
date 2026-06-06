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

export interface WritePumpOptions {
  budgetBytes?: number;
  sliceChars?: number;
  /** Macrotask yield between batches. Injectable so tests can drive the pump
   *  deterministically; defaults to `setTimeout(resolve, 0)`. */
  yieldToMacrotask?: () => Promise<void>;
}

export interface TerminalWritePump {
  /** Enqueue raw bytes (a PTY output chunk) for the throttled drain. */
  enqueue(data: Uint8Array): void;
  /** Slice a large string into budget-sized pieces (surrogate-safe) and
   *  enqueue them in order. Used for the one-shot disk scrollback restore. */
  enqueueString(data: string): void;
  /** Stop draining and drop anything still queued. Idempotent. After this,
   *  further `enqueue`/`enqueueString` calls are ignored. */
  cancel(): void;
  /** True once `cancel()` has been called. */
  readonly cancelled: boolean;
  /** Number of items still waiting to be written. Test/debug helper. */
  readonly pending: number;
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

  const queue: Array<Uint8Array | string> = [];
  let pumpRunning = false;
  let cancelled = false;

  const drain = async (): Promise<void> => {
    if (pumpRunning) return;
    pumpRunning = true;
    try {
      while (queue.length > 0 && !cancelled) {
        let budget = budgetBytes;
        while (queue.length > 0 && budget > 0 && !cancelled) {
          const next = queue.shift()!;
          budget -= next.length;
          write(next);
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
        queue.push(data.slice(i, end));
        i = end;
      }
      void drain();
    },
    cancel(): void {
      cancelled = true;
      queue.length = 0;
    },
    get cancelled(): boolean {
      return cancelled;
    },
    get pending(): number {
      return queue.length;
    },
  };
}
