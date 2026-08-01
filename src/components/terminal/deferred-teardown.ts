/**
 * Post-paint teardown queue for terminal panes.
 *
 * ── Why this exists ──
 * A workspace switch unmounts every terminal pane, and React runs the outgoing
 * pane's cleanup before the incoming pane can paint. Most of that cleanup is
 * cheap and has to stay synchronous (it stops input/output flow). The tail is
 * not: serializing the xterm buffer for scrollback persistence is a >30ms
 * main-thread block on a deep scrollback, and `term.dispose()` plus the WebGL
 * addon teardown are not free either. None of it is observable to the incoming
 * pane, so none of it needs to happen before the switch paints.
 *
 * This module parks that tail and runs it after the next paint, at idle:
 * double `requestAnimationFrame` (the second frame callback runs after the
 * commit that includes the incoming pane has been painted) then
 * `requestIdleCallback` with a timeout so a busy main thread can't starve it.
 *
 * ── The bounds, and why they are not negotiable ──
 * This repo has rolled back two separate attempts at keeping terminals alive
 * across switches (`14735bf` → `2baa42f`), for two specific failure modes:
 *
 *   1. Parked xterm instances that stayed subscribed to PTY output made every
 *      keystroke pay N× the render cost. A job parked here is already detached
 *      from the backend and its write pump is cancelled by the synchronous part
 *      of the cleanup, so a parked terminal consumes nothing — it only holds
 *      memory, and only until the next idle slot.
 *   2. Undisposed WebGL addons leaked GPU contexts until the browser's ~16
 *      context cap was exhausted and terminals rendered blank. The parked job
 *      still disposes the addon; the queue holds at most
 *      `MAX_PARKED_TEARDOWNS` jobs (a third park runs the oldest synchronously
 *      first), and TerminalPane flushes a session's parked job before creating
 *      a replacement Terminal for that same session. Extra live contexts are
 *      therefore capped at `MAX_PARKED_TEARDOWNS` and are transient.
 *
 * Everything here is a module-level singleton because the queue's whole job is
 * to bound work across panes that never see each other.
 */

/**
 * Handle returned by the injectable scheduler. `requestAnimationFrame` /
 * `requestIdleCallback` return a `number`; the `setTimeout` fallbacks return a
 * Node/DOM timer handle. The union covers both.
 */
export type TeardownHandle = number | ReturnType<typeof setTimeout>;

export interface DeferredTeardown {
  /** Session whose pane is tearing down. Used by `flushTeardown`. */
  sessionId: string;
  /** The parked work. Runs exactly once, and never throws to the caller. */
  run: () => void;
}

export interface TeardownScheduler {
  requestFrame: (cb: () => void) => TeardownHandle;
  cancelFrame: (handle: TeardownHandle) => void;
  requestIdle: (cb: () => void) => TeardownHandle;
  cancelIdle: (handle: TeardownHandle) => void;
}

/** A parked job holds a live xterm Terminal (buffers + a GPU context), so the
 *  bound is a memory bound, not a throughput one. Two covers the realistic
 *  worst case — a multi-pane workspace switch lands one job per visible pane
 *  and the third park drains the oldest synchronously. */
export const MAX_PARKED_TEARDOWNS = 2;

/** Upper bound on how long a parked job can wait for a genuine idle slot. */
const IDLE_TIMEOUT_MS = 500;
/** Fallback delay where `requestIdleCallback` is unavailable (older WebKitGTK,
 *  jsdom). Long enough to clear the switch, short enough to bound the park. */
const IDLE_FALLBACK_MS = 200;

// Feature-detect once at module load, same policy as the idle serializer: the
// request/cancel pair must match, so detect them together.
const hasRequestAnimationFrame =
  typeof requestAnimationFrame === "function" &&
  typeof cancelAnimationFrame === "function";
const hasRequestIdleCallback =
  typeof requestIdleCallback === "function" &&
  typeof cancelIdleCallback === "function";

const defaultScheduler: TeardownScheduler = {
  requestFrame: (cb) =>
    hasRequestAnimationFrame ? requestAnimationFrame(() => cb()) : setTimeout(cb, 0),
  cancelFrame: (handle) => {
    if (hasRequestAnimationFrame) cancelAnimationFrame(handle as number);
    else clearTimeout(handle as ReturnType<typeof setTimeout>);
  },
  requestIdle: (cb) =>
    hasRequestIdleCallback
      ? requestIdleCallback(() => cb(), { timeout: IDLE_TIMEOUT_MS })
      : setTimeout(cb, IDLE_FALLBACK_MS),
  cancelIdle: (handle) => {
    if (hasRequestIdleCallback) cancelIdleCallback(handle as number);
    else clearTimeout(handle as ReturnType<typeof setTimeout>);
  },
};

let scheduler: TeardownScheduler = defaultScheduler;

/** Swap the scheduler so tests can drive the rAF/idle chain deterministically.
 *  Passing `null` restores the feature-detected default. */
export function setTeardownScheduler(next: TeardownScheduler | null): void {
  scheduler = next ?? defaultScheduler;
}

interface ParkedJob {
  sessionId: string;
  run: () => void;
  ran: boolean;
  cancel: () => void;
}

// Oldest first, so eviction and `flushAllTeardowns` drain in park order.
const queue: ParkedJob[] = [];

function runJob(job: ParkedJob): void {
  // Exactly-once: the rAF/idle chain and an explicit flush can race, and a job
  // holds a Terminal that must not be disposed twice.
  if (job.ran) return;
  job.ran = true;
  const index = queue.indexOf(job);
  if (index !== -1) queue.splice(index, 1);
  job.cancel();
  try {
    job.run();
  } catch (err) {
    // A failed teardown must not take down the caller — in the flush path that
    // caller is a pane that is trying to mount.
    console.error("[codemux::terminal-teardown] parked teardown failed:", err);
  }
}

function schedule(job: ParkedJob): void {
  // Captured per job: a scheduler swap mid-park must still cancel with the
  // implementation that issued the handle.
  const owned = scheduler;
  let handle: TeardownHandle | null = null;
  let waitingOnFrame = true;

  job.cancel = () => {
    if (handle === null) return;
    if (waitingOnFrame) owned.cancelFrame(handle);
    else owned.cancelIdle(handle);
    handle = null;
  };

  // Double rAF: the first callback runs before the pending commit's paint, the
  // second after it — so the incoming pane is on screen before we start.
  handle = owned.requestFrame(() => {
    handle = owned.requestFrame(() => {
      waitingOnFrame = false;
      handle = owned.requestIdle(() => {
        handle = null;
        runJob(job);
      });
    });
  });
}

/**
 * Park a teardown to run after the next paint, at idle.
 *
 * Parking a third job runs the oldest synchronously first, so the number of
 * live-but-abandoned terminals is deterministically bounded.
 */
export function parkTeardown(job: DeferredTeardown): void {
  // A second job for the same session means a pane remounted without the
  // mount-side flush. Finish the older one first so its serialize/persist can
  // never land after the newer one's and resurrect stale scrollback.
  flushTeardown(job.sessionId);
  while (queue.length > 0 && queue.length >= MAX_PARKED_TEARDOWNS) {
    runJob(queue[0]);
  }
  const parked: ParkedJob = {
    sessionId: job.sessionId,
    run: job.run,
    ran: false,
    cancel: () => {},
  };
  queue.push(parked);
  schedule(parked);
}

/** Run this session's parked teardown synchronously now. Idempotent, and a
 *  no-op when nothing is parked for the session. */
export function flushTeardown(sessionId: string): void {
  for (let i = queue.length - 1; i >= 0; i--) {
    if (queue[i].sessionId === sessionId) runJob(queue[i]);
  }
}

/** Run every parked teardown synchronously now, in park order. Idempotent. */
export function flushAllTeardowns(): void {
  while (queue.length > 0) runJob(queue[0]);
}

/** Sessions with a teardown still parked, oldest first. Diagnostics + tests. */
export function pendingTeardownSessionIds(): string[] {
  return queue.map((job) => job.sessionId);
}
