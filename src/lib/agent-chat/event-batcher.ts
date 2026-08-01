import { useAgentChatStore } from "@/stores/agent-chat-store";
import type { ProviderRuntimeEvent } from "@/tauri/events";

import type { LiveChatEvent } from "./types";

/**
 * Display-cadence coalescer for provider runtime events.
 *
 * A streaming turn delivers `content_delta` tokens far faster than the
 * screen refreshes. Applied one at a time each token costs a store `set()`,
 * a full transcript-slot rebuild and a React commit — O(n) work per token
 * for a change the user could not perceive separately anyway. This layer
 * sits between the per-thread channel handler and the store and lets a
 * frame's worth of tokens land in ONE `applyEvents` call.
 *
 * The batching is deliberately lossless and order-preserving:
 *
 *  - Only `content_delta` is ever allowed to wait. Every other event kind
 *    (approval requests, turn/session state, tool results, …) flushes the
 *    thread's whole queue synchronously — itself included, in arrival order
 *    — so cross-event ordering and the latency of anything interactive are
 *    exactly what they were before batching.
 *  - Flushing runs on `requestAnimationFrame` while the document is
 *    visible. A hidden document does not fire rAF at all, so a coarse timer
 *    takes over there and a visibility change drains whatever is queued.
 *  - Callers flush explicitly at the seams the scheduler cannot see: a pane
 *    detaching from a thread, and just before a hydrate replaces the
 *    transcript wholesale.
 *
 * The queue doubles as the hydrate window's buffer. `hold` parks EVERY
 * kind (not just deltas) for one thread while a cursor hydrate is in
 * flight; `release` drains it afterwards. This is deliberate over-delivery
 * — the tail read and the buffered live events may overlap, and the
 * store's persisted-id guard drops the duplicates. The alternative
 * (applying live events while a tail read is in flight) loses them to the
 * merge instead, which is a gap, and gaps are not recoverable.
 *
 * A hold is OWNED: `hold` hands back a token that `release` / `drop` must
 * present. Panes remount faster than a hydrate's IPC round trip (a
 * workspace switch back and forth, or a dev double-mount), so two hydrates
 * for the same thread can overlap. The newer `hold` supersedes the older
 * token, and the older hydrate's teardown then no-ops instead of tearing
 * down the live hold — which would let events apply mid-read and leave the
 * newer hydrate's tail double-applying rows.
 *
 * Terminal output is a separate system (PTY channel → xterm write pump) and
 * does not pass through here.
 */

/** Fallback cadence while `document.hidden` — rAF is parked there. */
export const HIDDEN_FLUSH_INTERVAL_MS = 32;

/** A scheduler hands back its own cancel function. */
type Schedule = (run: () => void) => () => void;

/** Proof of ownership for a hold. Opaque on purpose: only the caller that
 *  took the hold can end it, so an older hydrate's teardown cannot tear
 *  down a newer one's. */
export type AgentChatHoldToken = symbol;

/** Last chance to edit a held buffer before it drains. The hydrate uses it
 *  to drop events the tail read already settled — see
 *  `cursor-hydrate.ts`. */
export type HeldEventFilter = (events: LiveChatEvent[]) => LiveChatEvent[];

export interface AgentChatEventBatcherOptions {
  /** Sink for a coalesced batch — the store's `applyLiveEvents` in
   *  production. */
  apply: (threadId: string, events: LiveChatEvent[]) => void;
  /** Injected by tests; defaults to `requestAnimationFrame`. */
  scheduleFrame?: Schedule;
  /** Injected by tests; defaults to a `HIDDEN_FLUSH_INTERVAL_MS` timer. */
  scheduleHiddenTimer?: Schedule;
  /** Injected by tests; defaults to `document.hidden`. */
  isHidden?: () => boolean;
  /** Injected by tests; defaults to `document`'s visibilitychange. */
  observeVisibility?: (onChange: () => void) => () => void;
}

export interface AgentChatEventBatcher {
  /** Queue one event; may flush synchronously (see the module doc). */
  enqueue: (
    threadId: string,
    event: ProviderRuntimeEvent,
    persistedId?: number | null,
  ) => void;
  /** Apply one thread's queued events now. No-op when its queue is empty
   *  or the thread is held. */
  flush: (threadId: string) => void;
  /** Apply every unheld thread's queued events now. */
  flushAll: () => void;
  /** Buffer one thread's events (all kinds) until `release` / `drop`.
   *  A second hold on the same thread supersedes the first: the returned
   *  token is the only one that can end it. */
  hold: (threadId: string) => AgentChatHoldToken;
  /** End a hold and apply whatever accumulated, optionally filtered.
   *  No-op unless `token` still owns the hold. */
  release: (
    threadId: string,
    token: AgentChatHoldToken,
    filter?: HeldEventFilter,
  ) => void;
  /** End a hold and DISCARD whatever accumulated. Safe only because the
   *  slice's cursor has not advanced past those events, so the next
   *  hydrate's tail read fetches them again. No-op unless `token` still
   *  owns the hold. */
  drop: (threadId: string, token: AgentChatHoldToken) => void;
  /** Whether a thread is currently held — diagnostics and tests. */
  isHeld: (threadId: string) => boolean;
  /** Events queued but not yet applied — diagnostics and tests. */
  pending: (threadId: string) => number;
  /** Drop the visibility listener. The shared batcher lives for the whole
   *  session, so this exists for tests and future teardown paths. */
  dispose: () => void;
}

export function createAgentChatEventBatcher(
  options: AgentChatEventBatcherOptions,
): AgentChatEventBatcher {
  const apply = options.apply;
  const isHidden = options.isHidden ?? defaultIsHidden;
  const scheduleFrame = options.scheduleFrame ?? defaultScheduleFrame;
  const scheduleHiddenTimer =
    options.scheduleHiddenTimer ?? defaultScheduleHiddenTimer;
  const observeVisibility = options.observeVisibility ?? defaultObserveVisibility;

  // Map iteration is insertion-ordered, so a multi-thread drain replays
  // threads in the order their first queued event arrived.
  const queues = new Map<string, LiveChatEvent[]>();
  const held = new Map<string, AgentChatHoldToken>();
  let cancelScheduled: (() => void) | null = null;

  const unschedule = () => {
    if (!cancelScheduled) return;
    cancelScheduled();
    cancelScheduled = null;
  };

  const schedule = () => {
    if (cancelScheduled) return;
    const run = () => {
      cancelScheduled = null;
      flushAll();
    };
    cancelScheduled = isHidden()
      ? scheduleHiddenTimer(run)
      : scheduleFrame(run);
  };

  /** Whether any queued thread could actually be drained right now. A queue
   *  belonging to a HELD thread cannot: only `release` / `drop` can move it. */
  const hasDrainableQueue = (): boolean => {
    for (const threadId of queues.keys()) {
      if (!held.has(threadId)) return true;
    }
    return false;
  };

  /** Arm the flush only when there is something a flush could do.
   *
   *  Scheduling on "any queue is non-empty" instead burns a frame callback
   *  per frame for as long as every queued thread stays held — i.e. for the
   *  whole of a hydrate's IPC round trip, which is exactly when the thread is
   *  held and exactly when the main thread is busiest. Each of those wakeups
   *  drains nothing and re-arms itself: a 60 Hz no-op loop. */
  const scheduleIfDrainable = () => {
    if (hasDrainableQueue()) schedule();
  };

  const flush = (threadId: string, filter?: HeldEventFilter) => {
    if (held.has(threadId)) return;
    const queued = queues.get(threadId);
    if (!queued || queued.length === 0) return;
    queues.delete(threadId);
    if (queues.size === 0) unschedule();
    const events = filter ? filter(queued) : queued;
    if (events.length === 0) return;
    apply(threadId, events);
  };

  const flushAll = () => {
    if (queues.size === 0) {
      unschedule();
      return;
    }
    const drained: Array<[string, LiveChatEvent[]]> = [];
    for (const [threadId, events] of queues) {
      if (held.has(threadId)) continue;
      drained.push([threadId, events]);
      queues.delete(threadId);
    }
    unschedule();
    for (const [threadId, events] of drained) apply(threadId, events);
    // A held thread keeps its queue, so it may still owe a drain — but not one
    // this scheduler can perform. Re-arming for it would spin until the hold
    // ends; the hold's own `release` / `drop` is what wakes it instead.
    scheduleIfDrainable();
  };

  const enqueue = (
    threadId: string,
    event: ProviderRuntimeEvent,
    persistedId: number | null = null,
  ) => {
    const item: LiveChatEvent = { event, persistedId };
    const queued = queues.get(threadId);
    if (queued) queued.push(item);
    else queues.set(threadId, [item]);
    if (held.has(threadId)) return;
    if (event.type === "content_delta") schedule();
    else flush(threadId);
  };

  const hold = (threadId: string): AgentChatHoldToken => {
    const token: AgentChatHoldToken = Symbol("agent-chat-hold");
    held.set(threadId, token);
    return token;
  };

  const release = (
    threadId: string,
    token: AgentChatHoldToken,
    filter?: HeldEventFilter,
  ) => {
    if (held.get(threadId) !== token) return;
    held.delete(threadId);
    flush(threadId, filter);
    // Ending a hold is the event that can make another thread's queue
    // drainable again, and `flushAll` deliberately stopped re-arming for
    // undrainable queues — so the wakeup has to come from here.
    scheduleIfDrainable();
  };

  const drop = (threadId: string, token: AgentChatHoldToken) => {
    if (held.get(threadId) !== token) return;
    held.delete(threadId);
    queues.delete(threadId);
    if (queues.size === 0) unschedule();
    else scheduleIfDrainable();
  };

  // A document going hidden parks rAF mid-flight; drain rather than stall
  // the queue until it comes back.
  const stopObserving = observeVisibility(() => {
    unschedule();
    flushAll();
  });

  return {
    enqueue,
    flush,
    flushAll,
    hold,
    release,
    drop,
    isHeld: (threadId) => held.has(threadId),
    pending: (threadId) => queues.get(threadId)?.length ?? 0,
    dispose: () => {
      stopObserving();
      unschedule();
      queues.clear();
      held.clear();
    },
  };
}

function defaultIsHidden(): boolean {
  return typeof document !== "undefined" && document.hidden;
}

function defaultScheduleFrame(run: () => void): () => void {
  if (typeof requestAnimationFrame !== "function") {
    return defaultScheduleHiddenTimer(run);
  }
  const handle = requestAnimationFrame(() => run());
  return () => cancelAnimationFrame(handle);
}

function defaultScheduleHiddenTimer(run: () => void): () => void {
  const handle = setTimeout(run, HIDDEN_FLUSH_INTERVAL_MS);
  return () => clearTimeout(handle);
}

function defaultObserveVisibility(onChange: () => void): () => void {
  if (typeof document === "undefined") return () => undefined;
  const listener = () => {
    if (document.hidden) onChange();
  };
  document.addEventListener("visibilitychange", listener);
  return () => document.removeEventListener("visibilitychange", listener);
}

// ---------------------------------------------------------------------------
// Shared instance
// ---------------------------------------------------------------------------

let shared: AgentChatEventBatcher | null = null;

function sharedBatcher(): AgentChatEventBatcher {
  shared ??= createAgentChatEventBatcher({
    apply: (threadId, events) =>
      useAgentChatStore.getState().applyLiveEvents(threadId, events),
  });
  return shared;
}

/** Route one live provider event through the shared coalescer.
 *  `persistedId` is the durable row the backend wrote for this event,
 *  when it wrote one — the store dedups replay overlap on it. */
export function enqueueAgentChatEvent(
  threadId: string,
  event: ProviderRuntimeEvent,
  persistedId: number | null = null,
): void {
  sharedBatcher().enqueue(threadId, event, persistedId);
}

/** Apply one thread's queued events immediately (detach / pre-hydrate). */
export function flushAgentChatEvents(threadId: string): void {
  sharedBatcher().flush(threadId);
}

/** Apply every queued event immediately. */
export function flushAllAgentChatEvents(): void {
  sharedBatcher().flushAll();
}

/** Buffer one thread's live events for the duration of a hydrate. The
 *  returned token is required to end the hold. */
export function holdAgentChatEvents(threadId: string): AgentChatHoldToken {
  return sharedBatcher().hold(threadId);
}

/** End a hold and apply what arrived during it, optionally filtered.
 *  No-op if a newer hold has superseded `token`. */
export function releaseAgentChatEvents(
  threadId: string,
  token: AgentChatHoldToken,
  filter?: HeldEventFilter,
): void {
  sharedBatcher().release(threadId, token, filter);
}

/** End a hold and discard what arrived during it (cancelled hydrate).
 *  The slice's cursor never advanced past those rows, so the next
 *  hydrate's tail read brings them back. No-op if a newer hold has
 *  superseded `token`. */
export function dropAgentChatEvents(
  threadId: string,
  token: AgentChatHoldToken,
): void {
  sharedBatcher().drop(threadId, token);
}
