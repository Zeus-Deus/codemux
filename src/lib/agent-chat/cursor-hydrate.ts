import { useAgentChatStore } from "@/stores/agent-chat-store";
import {
  agentChatListMessagesAfter,
  agentChatListMessagesBefore,
  agentChatListMessagesTail,
  agentChatThreadHeadId,
  agentChatTurnActive,
  type AgentChatMessageRow,
  type AgentChatMessageTail,
} from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";

import { waitForAgentChatAttach } from "./attach-registry";
import {
  dropAgentChatEvents,
  flushAgentChatEvents,
  holdAgentChatEvents,
  releaseAgentChatEvents,
  type HeldEventFilter,
} from "./event-batcher";
import { MAX_WARM_TAIL_ROWS } from "./hydrate";
import { endSubMeasure, startSubMeasure } from "@/lib/perf/interaction-trace";
import type { LiveChatEvent } from "./types";

/**
 * Resume one thread's transcript by cursor.
 *
 * The pane's live event stream is history-free (the per-thread channel
 * carries live events only), so a remount has to pull whatever the
 * backend persisted while it was gone. Doing that from row zero — and
 * comparing rendered message counts to decide whether to bother — is
 * what made a big thread cost ~100 ms of reduction on every visit.
 *
 * Instead the slice carries `lastPersistedEventId`, the durable id of
 * the last row it applied:
 *
 *  - **Warm** (cursor present): fetch only the rows after it and fold
 *    them into the existing state with ONE ordered reduction. An empty
 *    tail is the common case and does no work at all.
 *  - **Cold** (no cursor, cursor above the thread's head, or a tail
 *    past {@link MAX_WARM_TAIL_ROWS}): fetch the thread and replay it
 *    once, replacing the transcript wholesale. A LONG thread is opened
 *    tail-first — see "Tail-first cold open" below.
 *
 * ## Tail-first cold open
 *
 * A cold read of a 10k-row thread spends ~1.5 s moving 40 MB of payload
 * across the IPC before the reducer gets to run for 57 ms; the pane is
 * blank the whole time. The user only needs the newest turns to start
 * reading, so a cold open asks for the TAIL first ({@link TAIL_FIRST_ROWS}
 * rows, widened by the backend to a `user_message` boundary so the page
 * is whole turns), replays that at once through `hydrateThreadTail`, and
 * then backfills the older rows in pages while the user is already
 * looking at the bottom of the transcript.
 *
 * The backfill does NOT merge reduced items — it re-replays the COMPLETE
 * row set through one `hydrateThread`. The reducer is not additive across
 * a turn boundary in the other direction either (an item's post-passes
 * depend on what follows it), and 57 ms for the whole set is cheap.
 * `adoptItemIds` carries the visible rows' keys over so the transcript
 * list does not remount what the user is reading.
 *
 * Two invariants keep the slice honest across the two phases:
 *
 *  - the tail preview leaves `lastPersistedEventId` NULL. A non-null
 *    cursor promises "everything at or below is represented", and a warm
 *    revisit trusts that — with a tail-only slice it would never fetch
 *    the prefix. Only the final `hydrateThread` sets the cursor.
 *  - the hold stays in place until the final replay. Releasing between
 *    the phases would apply live events to a slice the backfill's
 *    replay is about to REPLACE wholesale, losing them.
 *
 * A cancellation between the phases drops the held events and leaves the
 * cursor null, so the next mount opens cold again from the tail.
 *
 * ## The attach/hydrate race
 *
 * Ordering is: hold → attach resolves → read → apply → filtered release.
 *
 * Live events for the thread are HELD from before the attach completes
 * until after the merge, then drained. That deliberately over-delivers: an
 * event can arrive both in the tail read and on the channel. The store
 * drops the duplicate by `persistedId`. The alternative — applying live
 * events while a read is in flight — loses them to the merge instead,
 * and a gap is not recoverable the way a duplicate is.
 *
 * The read waits for the attach because the backend has no
 * replay-on-attach: a row persisted while the attach is in flight is
 * delivered to no channel, and the next live event advances the cursor
 * past it. Reading after the attach makes the tail the authority for
 * everything up to that moment, and the held buffer the authority for
 * everything after it.
 *
 * A cancelled run DISCARDS the held events (the cursor never advanced
 * past them, so the next attempt's tail read fetches them again); a
 * FAILED run drops the cursor first, so the next attempt rebuilds cold
 * rather than trusting a position whose rows never arrived.
 *
 * The hold is OWNED (`hold` returns a token). Two hydrates for the same
 * thread can overlap — a pane remounts faster than the IPC round trip —
 * and the older one's teardown must not tear down the newer one's hold.
 *
 * ## Deliberately deferred
 *
 * A PERSISTED reduced projection (cold start without replaying rows at
 * all) is not part of this phase. Replay is not a pure function of the
 * rows: it stamps wall-clock times, mints ids from a module counter, and
 * takes tail-dependent post-passes — so a cached projection would need a
 * versioning scheme that the warm slice + cursor tail already make
 * unnecessary for the common case. The warm in-memory slice IS the
 * materialized thread for now.
 */
export interface CursorHydrateDeps {
  listAfter: (
    threadId: string,
    afterId: number | null,
  ) => Promise<AgentChatMessageRow[]>;
  headId: (threadId: string) => Promise<number | null>;
  turnActive: (
    provider: AgentChatProviderKind,
    threadId: string,
  ) => Promise<boolean>;
  /** Cold-open tail read: at least `limit` rows, widened to a user-turn
   *  boundary, with `complete` telling whether that was the whole thread.
   *  Optional so a caller (or older test) without it still gets the plain
   *  full read. */
  listTail?: (threadId: string, limit: number) => Promise<AgentChatMessageTail>;
  /** Backfill page: the newest `limit` rows strictly older than
   *  `beforeId`, ascending. A short page is the start of the thread. */
  listBefore?: (
    threadId: string,
    beforeId: number,
    limit: number,
  ) => Promise<AgentChatMessageRow[]>;
  /** Rows per backfill page; defaults to {@link BACKFILL_PAGE_ROWS}.
   *  Injected by tests to exercise the multi-page walk. */
  backfillPageRows?: number;
  /** Resolves once the thread's live channel is attached. Defaults to the
   *  attach registry the subscription hook writes. */
  awaitAttach?: (threadId: string) => Promise<void>;
  /** Backoff before the single re-home retry. Injected by tests. */
  delay?: (ms: number) => Promise<void>;
}

const defaultDeps: CursorHydrateDeps = {
  listAfter: agentChatListMessagesAfter,
  headId: agentChatThreadHeadId,
  turnActive: agentChatTurnActive,
  listTail: agentChatListMessagesTail,
  listBefore: agentChatListMessagesBefore,
};

/**
 * Minimum rows a cold open renders before anything else: a few turns of
 * a busy thread, enough to fill the viewport and then some. The backend
 * widens it down to the nearest `user_message` so the page replays clean.
 */
export const TAIL_FIRST_ROWS = 400;

/**
 * Rows per backfill page. Sized so one page is a few MB at most even on
 * a tool-heavy thread — the point of paging is that a giant single
 * response would block the IPC (and the renderer's main thread while it
 * deserialises) for the exact stretch the tail preview just freed.
 */
export const BACKFILL_PAGE_ROWS = 2_000;

/**
 * Probe the backend for "is this thread's run still in flight", returning
 * `null` — NOT `false` — when the question could not be answered.
 *
 * `agent_chat_turn_active` rejects for reasons that have nothing to do with
 * liveness: the agent-chat feature flag being off, and a
 * `provider_not_configured` registry miss that is reachable during startup.
 * Collapsing those into `false` told hydrate "the run is dead", which is the
 * one answer that flips a healthy transcript to "Run interrupted". `null`
 * lets the caller fall back to what the slice already knows instead.
 */
async function probeRunLive(
  deps: CursorHydrateDeps,
  provider: AgentChatProviderKind,
  threadId: string,
): Promise<boolean | null> {
  try {
    return await deps.turnActive(provider, threadId);
  } catch (err) {
    console.warn("[agent-chat] turn-active probe failed:", err);
    return null;
  }
}

/**
 * How many CONSECUTIVE unanswered probes a thread may carry its own
 * `streaming` forward through before the fallback gives up and settles.
 *
 * The fallback is a fixpoint without a bound: hydrate writes `streaming:
 * runLive`, so a slice that is already streaming re-derives `true` from
 * itself on every hydrate and can never settle while the probe keeps
 * rejecting — which it does for as long as the feature flag stays off or
 * the provider stays out of the registry. Three is chosen to cover the
 * startup window the probe's rejection modes actually live in (a couple of
 * remounts while the registry fills) without letting a permanently broken
 * probe pin a pane at "streaming" for the rest of the session.
 */
const MAX_UNANSWERED_PROBE_FALLBACKS = 3;

/** Consecutive unanswered probes per thread. Cleared by any definite
 *  answer, so a single transient rejection costs nothing. */
const unansweredProbes = new Map<string, number>();

/**
 * Resolve the probe's answer against the slice's own run state.
 *
 * Only an unanswered probe defers to the slice; a definite `false` is still
 * honoured, because that is how a genuinely-dead run earns its divider.
 *
 * The deferral is bounded — see {@link MAX_UNANSWERED_PROBE_FALLBACKS}.
 * Past the bound "no information" resolves to `false`, which is the
 * recoverable direction: the divider is wrong until the next real event,
 * whereas a stuck `streaming` hides the Continue chip indefinitely.
 */
function resolveRunLive(
  probed: boolean | null,
  threadId: string,
): boolean {
  if (probed !== null) {
    unansweredProbes.delete(threadId);
    return probed;
  }
  const seen = (unansweredProbes.get(threadId) ?? 0) + 1;
  unansweredProbes.set(threadId, seen);
  if (seen > MAX_UNANSWERED_PROBE_FALLBACKS) return false;
  return useAgentChatStore.getState().threads[threadId]?.streaming ?? false;
}

/**
 * Probe a thread's run liveness with the same "a rejection is no
 * information, not death" semantics hydrate uses.
 *
 * Exported for the turn-revert path, which rebuilds the transcript through
 * the same replay and so must not read a gating failure as a dead run.
 */
export async function resolveThreadRunLive(
  provider: AgentChatProviderKind,
  threadId: string,
  deps: CursorHydrateDeps = defaultDeps,
): Promise<boolean> {
  return resolveRunLive(await probeRunLive(deps, provider, threadId), threadId);
}

/**
 * Backoff for the one retry a cold read of zero rows over a NON-EMPTY
 * transcript earns. Long enough for a session collapse / thread re-home to
 * commit its row moves, short enough that the user does not notice.
 */
export const EMPTY_COLD_RETRY_MS = 500;

/**
 * Held-buffer filter for a release that follows an applied read.
 *
 * Deltas are never persisted, so the store's `persistedId` dedup cannot
 * see them — but the SETTLED item they streamed into is persisted, and a
 * read that returned it has already put the finished text on screen.
 * Replaying those deltas afterwards finds no streaming tail to merge into
 * and mints a SECOND assistant (or reasoning) block: the same text twice,
 * permanently.
 *
 * The discriminator is arrival order, not turn identity. Channel delivery
 * is ordered and the backend persists before it sends, so a held event
 * whose `persistedId` is at or below the just-applied cursor proves that
 * every event BEFORE it in the buffer is also already reflected in what
 * the read applied. Those deltas are dropped; everything after the last
 * such event is newer than the read and survives.
 *
 * Turn id alone would be wrong in both directions: one turn emits many
 * text blocks, so "this turn was settled" would discard the legitimate
 * deltas of a later block in the same turn.
 *
 * Dropping a delta can at worst lose a partial frame of streaming text —
 * its `item_completed` is still coming and rebuilds the full block —
 * whereas keeping a stale one duplicates content forever.
 */
export function dropDeltasSettledByRead(cursor: number): HeldEventFilter {
  return (events: LiveChatEvent[]) => {
    let lastCovered = -1;
    for (let i = 0; i < events.length; i += 1) {
      const persistedId = events[i].persistedId;
      if (persistedId != null && persistedId <= cursor) lastCovered = i;
    }
    if (lastCovered < 0) return events;
    return events.filter(
      (item, index) =>
        index > lastCovered || item.event.type !== "content_delta",
    );
  };
}

/** Per-call hooks for {@link hydrateThreadByCursor}. */
export interface CursorHydrateHooks {
  /** Fired the moment the slice holds something worth painting — after
   *  the tail preview of a tail-first open, or never for the other paths
   *  (where the promise resolving is that moment). The pane stamps its
   *  content-ready mark from here so the backfill is not counted against
   *  the time-to-content. */
  onContentReady?: () => void;
}

export async function hydrateThreadByCursor(
  threadId: string,
  provider: AgentChatProviderKind,
  isCancelled: () => boolean,
  deps: CursorHydrateDeps = defaultDeps,
  hooks: CursorHydrateHooks = {},
): Promise<void> {
  const retry = await hydratePass(threadId, provider, isCancelled, deps, hooks);
  if (!retry || isCancelled()) return;
  await (deps.delay ?? defaultDelay)(EMPTY_COLD_RETRY_MS);
  if (isCancelled()) return;
  // One retry only: a second empty read means the rows really are gone,
  // and the slice keeps what it has either way.
  await hydratePass(threadId, provider, isCancelled, deps, hooks, false);
}

/** One hold→read→apply→release cycle. Returns whether the caller should
 *  retry (see {@link EMPTY_COLD_RETRY_MS}). */
async function hydratePass(
  threadId: string,
  provider: AgentChatProviderKind,
  isCancelled: () => boolean,
  deps: CursorHydrateDeps,
  hooks: CursorHydrateHooks,
  allowRetry = true,
): Promise<boolean> {
  const onContentReady = hooks.onContentReady;
  // Drain what the coalescer already holds — those events are state the
  // slice owns — then park everything that arrives from here on.
  flushAgentChatEvents(threadId);
  const holdToken = holdAgentChatEvents(threadId);
  let releaseFilter: HeldEventFilter | undefined;
  let retry = false;
  try {
    // Read-after-attach: everything persisted up to this point is
    // guaranteed fetchable, everything after it lands in the held buffer.
    await (deps.awaitAttach ?? waitForAgentChatAttach)(threadId);
    if (isCancelled()) return false;
    const slice = useAgentChatStore.getState().threads[threadId];
    const cursor = slice?.lastPersistedEventId ?? null;
    const warm = slice != null && cursor != null;
    // A cold open of a long thread goes tail-first (see the module doc).
    // Both reads must be available; otherwise it is the plain full read.
    const tailFirst = !warm && deps.listTail != null && deps.listBefore != null;
    // The head probe rides alongside the tail read so a cursor from a
    // foreign id space (a merged / deleted thread whose rows were
    // re-homed) is caught without a full-history fetch.
    const readStarted = startSubMeasure();
    const [rows, headId, tail] = await Promise.all([
      tailFirst
        ? Promise.resolve([] as AgentChatMessageRow[])
        : deps.listAfter(threadId, warm ? cursor : null),
      warm ? deps.headId(threadId) : Promise.resolve(null),
      tailFirst
        ? deps.listTail!(threadId, TAIL_FIRST_ROWS)
        : Promise.resolve<AgentChatMessageTail | null>(null),
    ]);
    // A tail that turned out to be the whole thread IS the full read, and
    // is measured as one so the before/after numbers stay comparable.
    endSubMeasure(
      warm
        ? "hydrate:read-tail"
        : tail != null && !tail.complete
          ? `hydrate:read-tail-first(${tail.rows.length}/${tail.total_rows})`
          : "hydrate:read-full",
      readStarted,
    );
    if (isCancelled()) return false;
    const cursorAhead =
      warm && cursor !== 0 && (headId === null || headId < cursor);
    if (warm && !cursorAhead && rows.length <= MAX_WARM_TAIL_ROWS) {
      if (rows.length === 0) return false; // Warm and unchanged — zero work.
      // Ask the backend whether this thread's RUN is still in flight —
      // which includes a parent turn that yielded while delegated agents
      // keep working (see `agent_chat_turn_active`). A live run means the
      // tail's missing terminal event is expected, not an interrupt.
      //
      // Rows are fetched FIRST, liveness SECOND. That ordering is NOT
      // free of races in either direction: a turn that settles inside the
      // probe's IPC window persists its `turn_completed` after the row
      // read, so the tail looks unsettled AND the probe says false. What
      // makes it safe is the held-event buffer, not the ordering — that
      // `turn_completed` is sitting in the hold and is released into the
      // reducer moments later, which settles the thread. Both orderings
      // self-heal the same way; this one is kept only because it takes the
      // liveness reading as late as possible.
      const runLive = await resolveThreadRunLive(provider, threadId, deps);
      if (isCancelled()) return false;
      useAgentChatStore
        .getState()
        .applyPayloadsTail(threadId, rows, { runLive, provider });
      releaseFilter = dropDeltasSettledByRead(rows[rows.length - 1].id);
      return false;
    }
    if (tail != null && !tail.complete && tail.rows.length > 0) {
      releaseFilter = await hydrateTailFirst(
        threadId,
        provider,
        isCancelled,
        deps,
        tail,
        onContentReady,
      );
      return false;
    }
    const full = tail != null ? tail.rows : warm ? await deps.listAfter(threadId, null) : rows;
    if (isCancelled()) return false;
    // A cold read of NOTHING over a transcript that has messages is not
    // an empty thread — it is a thread whose rows are not (yet) filed
    // under this id. Session resume paints the picked chat's history
    // under a brand-new thread id and the backend re-homes the rows
    // afterwards, so a hydrate landing in that window reads zero.
    // Replacing the transcript with `[]` there erases what the user is
    // looking at. Keep the slice, leave the cursor invalidated so the
    // next visit still rebuilds cold, and retry once.
    const local = useAgentChatStore.getState().threads[threadId];
    if (full.length === 0 && (local?.messages.length ?? 0) > 0) {
      useAgentChatStore.getState().invalidateThreadCursor(threadId);
      retry = allowRetry;
      return retry;
    }
    const liveStarted = startSubMeasure();
    const runLive = await resolveThreadRunLive(provider, threadId, deps);
    endSubMeasure("hydrate:run-live-probe", liveStarted);
    if (isCancelled()) return false;
    const applyStarted = startSubMeasure();
    useAgentChatStore
      .getState()
      .hydrateThread(threadId, full, { runLive, provider });
    endSubMeasure(`hydrate:replay(${full.length})`, applyStarted);
    if (full.length > 0) {
      releaseFilter = dropDeltasSettledByRead(full[full.length - 1].id);
    }
    return false;
  } catch (err) {
    // Soft-fail: the user still sees whatever the live stream brings in.
    // Invalidate the cursor first — the events about to be released may
    // leave the slice short of rows we never fetched.
    useAgentChatStore.getState().invalidateThreadCursor(threadId);
    console.warn("[agent-chat] hydrate-on-mount failed:", err);
    return false;
  } finally {
    if (isCancelled()) {
      dropAgentChatEvents(threadId, holdToken);
    } else {
      releaseAgentChatEvents(threadId, holdToken, releaseFilter);
      // Opportunistic, never on a timer: this is the moment a thread
      // just got (or stayed) warm. Skipped for a pass that is about to
      // retry — the slice it would protect is the one still in doubt.
      if (!retry) useAgentChatStore.getState().evictColdThreads([threadId]);
    }
  }
}

/**
 * The two-phase body of a tail-first cold open (module doc: "Tail-first
 * cold open"). Runs INSIDE `hydratePass`'s hold: the caller owns the
 * token and releases (or drops, on cancel) in its `finally`, so this only
 * has to hand back the release filter — `undefined` when the run did not
 * reach the final replay.
 *
 * Phase 1 replays the tail as a preview and fires `onContentReady`.
 * Phase 2 pages the older rows down from the tail's first id until a
 * page comes back short, then re-replays the WHOLE set once and syncs
 * the cursor to the head. A cancel anywhere in phase 2 leaves the slice
 * as the preview with a null cursor — cold, so the next mount starts
 * over rather than trusting a half-fetched history.
 */
async function hydrateTailFirst(
  threadId: string,
  provider: AgentChatProviderKind,
  isCancelled: () => boolean,
  deps: CursorHydrateDeps,
  tail: AgentChatMessageTail,
  onContentReady: (() => void) | undefined,
): Promise<HeldEventFilter | undefined> {
  const liveStarted = startSubMeasure();
  const runLive = await resolveThreadRunLive(provider, threadId, deps);
  endSubMeasure("hydrate:run-live-probe", liveStarted);
  if (isCancelled()) return undefined;

  const previewStarted = startSubMeasure();
  useAgentChatStore
    .getState()
    .hydrateThreadTail(threadId, tail.rows, { runLive, provider });
  endSubMeasure(`hydrate:replay(${tail.rows.length})`, previewStarted);
  onContentReady?.();

  // Newest page first, so the pages are collected in reverse and flipped
  // when assembled. Each page's first row is the next page's exclusive
  // upper bound; a short (or empty) page is the start of the thread.
  const backfillStarted = startSubMeasure();
  const pageRows = deps.backfillPageRows ?? BACKFILL_PAGE_ROWS;
  const pagesNewestFirst: AgentChatMessageRow[][] = [];
  let beforeId = tail.rows[0].id;
  let fetched = 0;
  for (;;) {
    const page = await deps.listBefore!(threadId, beforeId, pageRows);
    if (isCancelled()) {
      // Nothing below the tail is represented, and the preview's cursor
      // is already null — but say so explicitly: a cancelled backfill
      // must never leave a slice that looks warm.
      useAgentChatStore.getState().invalidateThreadCursor(threadId);
      return undefined;
    }
    if (page.length === 0) break;
    pagesNewestFirst.push(page);
    fetched += page.length;
    if (page.length < pageRows) break;
    beforeId = page[0].id;
  }
  endSubMeasure(
    `hydrate:backfill(${fetched}/${pagesNewestFirst.length}p)`,
    backfillStarted,
  );

  const all: AgentChatMessageRow[] = [];
  for (let i = pagesNewestFirst.length - 1; i >= 0; i -= 1) {
    for (const row of pagesNewestFirst[i]) all.push(row);
  }
  for (const row of tail.rows) all.push(row);

  // One replay of the complete set. `hydrateThread` adopts the preview's
  // item ids for the suffix it re-creates, so the rows on screen keep
  // their keys and the history slides in above them. The liveness answer
  // from before the backfill is reused: anything that settled meanwhile
  // is sitting in the held buffer and lands right after this.
  const replayStarted = startSubMeasure();
  useAgentChatStore
    .getState()
    .hydrateThread(threadId, all, { runLive, provider });
  endSubMeasure(`hydrate:backfill-replay(${all.length})`, replayStarted);
  return dropDeltasSettledByRead(all[all.length - 1].id);
}

function defaultDelay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
