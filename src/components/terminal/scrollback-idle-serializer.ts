/**
 * Opportunistic idle-time scrollback serializer for a single xterm pane.
 *
 * ── Why this exists ──
 * Switching away from a workspace unmounts every terminal pane. TerminalPane's
 * unmount cleanup serializes the xterm buffer (`serializeAddon.serialize()`) so
 * the scrollback survives the React lifecycle and can be restored on return.
 * That serialize is synchronous main-thread work and, for plain shells with a
 * deep scrollback, is the dominant cost of the switch — the known >30ms hotspot
 * (issue #128). Alt-screen panes already skip it; the exposure is ordinary
 * shells that have accumulated scrollback.
 *
 * ── The idea ──
 * Instead of paying the whole serialize on the latency-critical unmount path,
 * do it opportunistically while the pane is idle and the user isn't switching.
 * We watch the byte stream: after output has been quiet for `settleMs` we take
 * one serialize during a `requestIdleCallback` slot and cache the result. When
 * the pane later unmounts, if that cache is still up to date we hand it straight
 * to the payload builder and skip the serialize entirely — the switch pays
 * nothing. If the cache is stale (output arrived since, or we're mid-serialize
 * cadence) the caller falls back to today's synchronous serialize, so
 * persistence semantics never regress.
 *
 * ── The keystroke-echo constraint ──
 * `notifyOutput()` runs for EVERY chunk written to xterm, including the
 * per-keystroke local echo. This repo has reverted three separate perf changes
 * for typing-latency regressions, so that path must stay O(1): a few field
 * assignments and a single branch. In particular we do NOT clearTimeout /
 * setTimeout on every chunk — we arm the settle-check timer only when one isn't
 * already pending and let it re-arm itself for the remaining quiet window. The
 * timer reads `lastOutputAt` to decide whether output has actually settled.
 *
 * ── Concurrency ──
 * JS is single-threaded and `serialize()` is synchronous, so no PTY output can
 * interleave with a serialize in progress — no locking is needed. We only have
 * to re-check staleness at the START of the idle callback, because output may
 * have arrived between scheduling the callback and it running.
 */

/**
 * Handle returned by the injectable idle scheduler. `requestIdleCallback`
 * returns a `number`; the `setTimeout` fallback returns a Node/DOM timer
 * handle. The union covers both so `cancelIdle` can accept whatever
 * `requestIdle` produced.
 */
export type IdleHandle = number | ReturnType<typeof setTimeout>;

export interface IdleScrollbackSerializerOptions {
  /**
   * The expensive serialize call. The caller wraps
   * `serializeAddon.serialize({ scrollback })`. This module only ever invokes
   * it when the buffer has settled AND is not on the alternate screen, so the
   * returned string is always primary-screen content.
   */
  serialize: () => string;
  /** True while xterm is showing the alternate screen (vim, htop, Claude Code,
   *  …). Alt-screen serializations are garbage and are never cached/restored. */
  isAlternateBuffer: () => boolean;
  /** Output must be quiet this long before we serialize. Default 1000ms. */
  settleMs?: number;
  /** At most one idle serialize per this interval, to bound background CPU for
   *  shells that emit in periodic bursts. Default 5000ms. */
  minIntervalMs?: number;
  /**
   * Gate checked at the start of every idle callback. When it returns false
   * (session restore disabled) the idle serialize is skipped WITHOUT
   * rescheduling: the result would be discarded anyway, so serializing is
   * pure wasted main-thread work, and not re-arming means zero timer churn
   * while disabled. We stay dirty; the next output re-arms the settle timer,
   * so re-enabling the setting self-heals. Correctness is unaffected because
   * the unmount persist path is gated on the same setting. Default: enabled.
   */
  isEnabled?: () => boolean;
  /** Clock source. Injectable for tests; defaults to `performance.now`. */
  now?: () => number;
  /**
   * Schedule an idle callback. The callback receives `didTimeout` — true when
   * the slot was granted only because the scheduler's timeout elapsed (the
   * main thread never actually went idle); `onIdle` uses it for the capped
   * contention defer. Defaults to `requestIdleCallback` (with a ~1000ms
   * timeout so a busy main thread can't starve it forever) when available —
   * WebKitGTK only gained rIC recently — falling back to `setTimeout(fn, 200)`,
   * which always reports `didTimeout: false`.
   * Injectable so tests drive it deterministically with fake timers.
   */
  requestIdle?: (cb: (didTimeout: boolean) => void) => IdleHandle;
  /** Cancel a handle returned by `requestIdle`. Must be paired with it. */
  cancelIdle?: (handle: IdleHandle) => void;
}

export interface IdleScrollbackSerializer {
  /** Called for EVERY chunk written to xterm (incl. per-keystroke echo). O(1). */
  notifyOutput(): void;
  /** Called on xterm resize — reflow invalidates the cached serialization. */
  notifyResize(): void;
  /** The cached serialization iff it is still current (clean cache taken from
   *  the primary screen and we're not on the alt screen right now); else null. */
  getFreshData(): { data: string } | null;
  /** Cancel pending timer + idle callback. Idempotent. */
  dispose(): void;
}

const DEFAULT_SETTLE_MS = 1000;
const DEFAULT_MIN_INTERVAL_MS = 5000;

// Feature-detect rIC once at module load. jsdom (our test env) and older
// WebKitGTK lack it, so we transparently fall back to a short setTimeout. The
// pair must match: if we scheduled via setTimeout we must clear via clearTimeout.
const hasRequestIdleCallback =
  typeof requestIdleCallback === "function" &&
  typeof cancelIdleCallback === "function";

function defaultRequestIdle(cb: (didTimeout: boolean) => void): IdleHandle {
  if (hasRequestIdleCallback) {
    // Forward the IdleDeadline's didTimeout so onIdle can tell a genuine
    // idle slot from a timeout-forced one. The timeout bounds worst-case
    // latency so a permanently busy main thread still (eventually) serializes.
    return requestIdleCallback((deadline) => cb(deadline.didTimeout), {
      timeout: 1000,
    });
  }
  // Fallback: a modest delay approximates "when the main thread is quiet".
  // There is no deadline to consult, so report a genuine idle slot.
  return setTimeout(() => cb(false), 200);
}

function defaultCancelIdle(handle: IdleHandle): void {
  if (hasRequestIdleCallback) {
    cancelIdleCallback(handle as number);
  } else {
    clearTimeout(handle as ReturnType<typeof setTimeout>);
  }
}

export function createIdleScrollbackSerializer(
  options: IdleScrollbackSerializerOptions,
): IdleScrollbackSerializer {
  const { serialize, isAlternateBuffer } = options;
  const settleMs = options.settleMs ?? DEFAULT_SETTLE_MS;
  const minIntervalMs = options.minIntervalMs ?? DEFAULT_MIN_INTERVAL_MS;
  const isEnabled = options.isEnabled ?? (() => true);
  const now = options.now ?? (() => performance.now());
  const requestIdle = options.requestIdle ?? defaultRequestIdle;
  const cancelIdle = options.cancelIdle ?? defaultCancelIdle;

  // Cleanliness is derived from `cachedData`: null ⇔ dirty (no current
  // serialization). Starts null, so getFreshData returns null until the first
  // successful idle serialize and the unmount path falls back to a
  // synchronous serialize.
  let cachedData: string | null = null;
  let lastOutputAt = 0;
  // -Infinity so the very first serialize is never blocked by the min-interval.
  let lastSerializeAt = -Infinity;
  // Consecutive idle callbacks granted only via the scheduler timeout — see
  // the capped contention defer in onIdle.
  let consecutiveTimeoutDefers = 0;

  let settleTimer: ReturnType<typeof setTimeout> | null = null;
  let idleHandle: IdleHandle | null = null;
  let disposed = false;

  // Arm the settle-check timer, but only if one isn't already pending — this is
  // what keeps notifyOutput O(1) (no clearTimeout/setTimeout churn per chunk).
  // Callers that need a specific remaining delay pass it; the guard makes the
  // call safe from anywhere.
  const armSettle = (delay: number): void => {
    if (settleTimer !== null || disposed) return;
    settleTimer = setTimeout(onSettleTimer, delay);
  };

  // Fired `settleMs` after output was last seen (best effort — the timer may
  // have been armed earlier and coalesced multiple chunks). Decide whether the
  // buffer is actually settled and, if so, hand off to the idle callback.
  function onSettleTimer(): void {
    settleTimer = null;
    if (disposed || cachedData !== null) return;

    const t = now();
    const sinceOutput = t - lastOutputAt;
    if (sinceOutput < settleMs) {
      // More output arrived after this timer was armed — wait out the remainder
      // of the quiet window rather than serializing into a moving buffer.
      armSettle(settleMs - sinceOutput);
      return;
    }
    const sinceSerialize = t - lastSerializeAt;
    if (sinceSerialize < minIntervalMs) {
      // Settled, but we serialized too recently. Re-arm for the remaining
      // interval so bursty shells can't drive back-to-back serializes.
      armSettle(minIntervalMs - sinceSerialize);
      return;
    }

    if (idleHandle === null) {
      idleHandle = requestIdle(onIdle);
    }
  }

  // Runs in an idle slot. Re-check everything: output may have arrived since
  // we scheduled, the pane may have entered the alt screen, or session
  // restore may have been switched off.
  function onIdle(didTimeout: boolean): void {
    idleHandle = null;
    if (disposed || cachedData !== null) return;

    if (!isEnabled()) {
      // Session restore is off: the serialization would be discarded on
      // unmount, so don't pay for it. Don't reschedule either — no timer
      // churn while disabled. We stay dirty (cachedData === null) and the
      // next output re-arms the settle timer, so re-enabling the setting
      // self-heals. Correctness is unaffected: the unmount persist path is
      // gated on the same setting.
      return;
    }

    if (didTimeout && consecutiveTimeoutDefers < 2) {
      // The idle slot was granted only because the scheduler timeout elapsed
      // — the main thread is busy, and serializing now would land
      // mid-contention. Defer and retry after another settle window. The
      // defer is CAPPED: an unconditional defer would starve under sustained
      // load and push the cost back onto the switch path (the very hotspot
      // this module exists to relieve), so after two deferrals we take the
      // bounded hit.
      consecutiveTimeoutDefers++;
      armSettle(settleMs);
      return;
    }
    consecutiveTimeoutDefers = 0;

    const sinceOutput = now() - lastOutputAt;
    if (sinceOutput < settleMs) {
      // Output landed between scheduling and running — go back to waiting.
      armSettle(settleMs - sinceOutput);
      return;
    }
    if (isAlternateBuffer()) {
      // Alt-screen content serializes to garbage and is never restored, so
      // skip it. We stay dirty; leaving the alt screen emits output, which
      // re-arms the settle timer and gives us a fresh chance on the primary buf.
      return;
    }

    try {
      cachedData = serialize();
      lastSerializeAt = now();
    } catch (err) {
      // Serialize threw (e.g. addon in a bad state). cachedData stays null
      // (dirty), so the unmount path falls back to today's synchronous
      // serialize — behavior unchanged. We don't self-re-arm here (that
      // could busy-loop on a persistent failure); the next output/resize
      // re-arms and retries.
      console.error(
        "[codemux::scrollback-idle] idle serialize failed, staying dirty:",
        err,
      );
    }
  }

  // Shared by notifyOutput and notifyResize: both invalidate the cache and mark
  // the buffer dirty. Kept to a handful of field writes + one branch so the
  // keystroke-echo path stays cheap.
  const markDirtyAndArm = (): void => {
    if (disposed) return;
    cachedData = null;
    lastOutputAt = now();
    // armSettle arms ONLY if no settle check is pending — the pending timer
    // re-arms itself for the remaining quiet window using the updated
    // lastOutputAt — which is what keeps this O(1) per chunk.
    armSettle(settleMs);
  };

  return {
    notifyOutput(): void {
      markDirtyAndArm();
    },
    notifyResize(): void {
      // A resize reflows the buffer, so any cached serialization is stale.
      markDirtyAndArm();
    },
    getFreshData(): { data: string } | null {
      // Only hand back the cache when it is provably current: a clean cache
      // exists (cachedData !== null ⇔ not dirty) and we're on the primary
      // screen right now (the cache is only ever taken from the primary
      // screen, and an alt-screen unmount must persist with alternate_buffer
      // semantics, not the stale primary cache).
      if (cachedData === null || isAlternateBuffer()) return null;
      return { data: cachedData };
    },
    dispose(): void {
      disposed = true;
      if (settleTimer !== null) {
        clearTimeout(settleTimer);
        settleTimer = null;
      }
      if (idleHandle !== null) {
        cancelIdle(idleHandle);
        idleHandle = null;
      }
    },
  };
}
