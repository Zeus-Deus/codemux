import { useAgentChatStore } from "@/stores/agent-chat-store";
import {
  agentChatListMessagesAfter,
  agentChatThreadHeadId,
  agentChatTurnActive,
  type AgentChatMessageRow,
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
 *    once, replacing the transcript wholesale.
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
};

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

export async function hydrateThreadByCursor(
  threadId: string,
  provider: AgentChatProviderKind,
  isCancelled: () => boolean,
  deps: CursorHydrateDeps = defaultDeps,
): Promise<void> {
  const retry = await hydratePass(threadId, provider, isCancelled, deps);
  if (!retry || isCancelled()) return;
  await (deps.delay ?? defaultDelay)(EMPTY_COLD_RETRY_MS);
  if (isCancelled()) return;
  // One retry only: a second empty read means the rows really are gone,
  // and the slice keeps what it has either way.
  await hydratePass(threadId, provider, isCancelled, deps, false);
}

/** One hold→read→apply→release cycle. Returns whether the caller should
 *  retry (see {@link EMPTY_COLD_RETRY_MS}). */
async function hydratePass(
  threadId: string,
  provider: AgentChatProviderKind,
  isCancelled: () => boolean,
  deps: CursorHydrateDeps,
  allowRetry = true,
): Promise<boolean> {
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
    // The head probe rides alongside the tail read so a cursor from a
    // foreign id space (a merged / deleted thread whose rows were
    // re-homed) is caught without a full-history fetch.
    const [rows, headId] = await Promise.all([
      deps.listAfter(threadId, warm ? cursor : null),
      warm ? deps.headId(threadId) : Promise.resolve(null),
    ]);
    if (isCancelled()) return false;
    const cursorAhead =
      warm && cursor !== 0 && (headId === null || headId < cursor);
    if (warm && !cursorAhead && rows.length <= MAX_WARM_TAIL_ROWS) {
      if (rows.length === 0) return false; // Warm and unchanged — zero work.
      // Ask the backend whether this thread's turn is still in flight.
      // Ordering matters: rows are fetched FIRST, liveness SECOND — if
      // the turn settles between the two calls the `turn_completed` row
      // is already in `rows`, so `runLive` can only ever be a
      // conservative false, never a stale "alive". A live turn means the
      // tail's missing terminal event is expected, not an interrupt.
      const runLive = await deps
        .turnActive(provider, threadId)
        .catch(() => false);
      if (isCancelled()) return false;
      useAgentChatStore
        .getState()
        .applyPayloadsTail(threadId, rows, { runLive, provider });
      releaseFilter = dropDeltasSettledByRead(rows[rows.length - 1].id);
      return false;
    }
    const full = warm ? await deps.listAfter(threadId, null) : rows;
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
    const runLive = await deps.turnActive(provider, threadId).catch(() => false);
    if (isCancelled()) return false;
    useAgentChatStore
      .getState()
      .hydrateThread(threadId, full, { runLive, provider });
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

function defaultDelay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
