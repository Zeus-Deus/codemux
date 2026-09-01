import { beforeEach, describe, expect, it, vi } from "vitest";

import { useAgentChatStore } from "@/stores/agent-chat-store";
import type { AgentChatMessageRow } from "@/tauri/commands";
import type { ProviderRuntimeEvent } from "@/tauri/events";

import { hydrateThreadByCursor, type CursorHydrateDeps } from "./cursor-hydrate";
import { enqueueAgentChatEvent, flushAllAgentChatEvents } from "./event-batcher";

/**
 * The attach/hydrate/live-drain race.
 *
 * A remounting pane attaches its live channel and reads the persisted
 * tail concurrently. Events can land in three windows:
 *
 *   1. BEFORE the hydrate reads the slice,
 *   2. after the read but before the merge applies,
 *   3. after the merge.
 *
 * Every ordering must end with each event applied EXACTLY ONCE (no
 * duplicate from the replay overlap) and NOTHING missing (no gap from
 * the window where no channel was attached). These tests drive the real
 * store, the real coalescer and the real hydrate orchestration; only the
 * IPC reads are faked.
 */

const THREAD = "thread-race";
const PROVIDER = "claude" as const;

/** A persisted, transcript-affecting event: one assistant message. */
function assistantEvent(text: string, turnId = "turn-1"): ProviderRuntimeEvent {
  return {
    type: "item_completed",
    thread_id: THREAD,
    turn_id: turnId,
    item: { kind: "assistant_text", text },
  } as ProviderRuntimeEvent;
}

function row(id: number, event: ProviderRuntimeEvent): AgentChatMessageRow {
  return { id, payload: JSON.stringify(event) };
}

function userRow(id: number, text: string, clientNonce?: string): AgentChatMessageRow {
  return {
    id,
    payload: JSON.stringify({
      type: "user_message",
      thread_id: THREAD,
      text,
      ...(clientNonce ? { client_nonce: clientNonce } : {}),
    }),
  };
}

/** The rendered assistant/user texts, in order. */
function transcript(threadId = THREAD): string[] {
  const slice = useAgentChatStore.getState().threads[threadId];
  if (!slice) return [];
  return slice.messages.flatMap((m) =>
    m.kind === "assistant_message" || m.kind === "user_message" ? [m.text] : [],
  );
}

/** A deps set whose tail read resolves only when the test says so. */
function deferredDeps(rows: AgentChatMessageRow[], headId: number | null) {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  const deps: CursorHydrateDeps = {
    listAfter: async (_threadId, afterId) => {
      await gate;
      return afterId === null ? rows : rows.filter((r) => r.id > afterId);
    },
    headId: async () => headId,
    turnActive: async () => false,
  };
  return { deps, release };
}

function seedWarmSlice(cursor: number, seedText: string) {
  useAgentChatStore.getState().hydrateThread(THREAD, [
    { id: cursor, payload: JSON.stringify(assistantEvent(seedText, "turn-0")) },
  ]);
  expect(
    useAgentChatStore.getState().threads[THREAD].lastPersistedEventId,
  ).toBe(cursor);
}

describe("cursor hydrate — attach/hydrate/live-drain race", () => {
  beforeEach(() => {
    useAgentChatStore.setState({ threads: {} });
  });

  it("ordering 1: an event applied BEFORE the read is not replayed by the tail", () => {
    // The event was already folded in and advanced the cursor, so the
    // tail read starts above it — the classic duplicate is impossible.
    seedWarmSlice(10, "seed");
    enqueueAgentChatEvent(THREAD, assistantEvent("live-11"), 11);
    const slice = useAgentChatStore.getState().threads[THREAD];
    expect(slice.lastPersistedEventId).toBe(11);
    expect(transcript()).toEqual(["seed", "live-11"]);
  });

  it("ordering 1 (cont.): the tail read starts from the advanced cursor", async () => {
    seedWarmSlice(10, "seed");
    enqueueAgentChatEvent(THREAD, assistantEvent("live-11"), 11);
    const listAfter = vi
      .fn<CursorHydrateDeps["listAfter"]>()
      .mockResolvedValue([]);
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, {
      listAfter,
      headId: async () => 11,
      turnActive: async () => false,
    });
    expect(listAfter).toHaveBeenCalledWith(THREAD, 11);
    expect(transcript()).toEqual(["seed", "live-11"]);
  });

  it("ordering 2: an event arriving mid-read is held, then deduped against the tail", async () => {
    // The drop window: rows 11 and 12 were persisted while the pane was
    // away, and 12 ALSO arrives live while the read is in flight. The
    // tail must supply 11 (a gap otherwise) and 12 must render once.
    seedWarmSlice(10, "seed");
    const { deps, release } = deferredDeps(
      [row(11, assistantEvent("row-11")), row(12, assistantEvent("row-12"))],
      12,
    );
    const running = hydrateThreadByCursor(THREAD, PROVIDER, () => false, deps);

    // Live delivery of the same event the tail read will return.
    enqueueAgentChatEvent(THREAD, assistantEvent("row-12"), 12);
    // Held: nothing applied yet.
    expect(transcript()).toEqual(["seed"]);

    release();
    await running;

    expect(transcript()).toEqual(["seed", "row-11", "row-12"]);
    expect(
      useAgentChatStore.getState().threads[THREAD].lastPersistedEventId,
    ).toBe(12);
  });

  it("ordering 2b: a held event ABOVE the tail still lands", async () => {
    // Overlap is not the only case — an event newer than everything the
    // read returned must survive the drain, or it is a gap.
    seedWarmSlice(10, "seed");
    const { deps, release } = deferredDeps([row(11, assistantEvent("row-11"))], 11);
    const running = hydrateThreadByCursor(THREAD, PROVIDER, () => false, deps);
    enqueueAgentChatEvent(THREAD, assistantEvent("live-12"), 12);
    release();
    await running;

    expect(transcript()).toEqual(["seed", "row-11", "live-12"]);
    expect(
      useAgentChatStore.getState().threads[THREAD].lastPersistedEventId,
    ).toBe(12);
  });

  it("ordering 3: an event delivered after the merge applies once and advances the cursor", async () => {
    seedWarmSlice(10, "seed");
    const { deps, release } = deferredDeps([row(11, assistantEvent("row-11"))], 11);
    const running = hydrateThreadByCursor(THREAD, PROVIDER, () => false, deps);
    release();
    await running;

    enqueueAgentChatEvent(THREAD, assistantEvent("live-12"), 12);
    expect(transcript()).toEqual(["seed", "row-11", "live-12"]);
    expect(
      useAgentChatStore.getState().threads[THREAD].lastPersistedEventId,
    ).toBe(12);
  });

  it("all three windows at once, with the whole tail overlapping the live stream", async () => {
    seedWarmSlice(10, "seed");
    enqueueAgentChatEvent(THREAD, assistantEvent("e-11"), 11); // window 1
    const { deps, release } = deferredDeps(
      [
        row(11, assistantEvent("e-11")),
        row(12, assistantEvent("e-12")),
        row(13, assistantEvent("e-13")),
      ],
      13,
    );
    const running = hydrateThreadByCursor(THREAD, PROVIDER, () => false, deps);
    enqueueAgentChatEvent(THREAD, assistantEvent("e-12"), 12); // window 2
    enqueueAgentChatEvent(THREAD, assistantEvent("e-13"), 13); // window 2
    release();
    await running;
    enqueueAgentChatEvent(THREAD, assistantEvent("e-14"), 14); // window 3

    expect(transcript()).toEqual(["seed", "e-11", "e-12", "e-13", "e-14"]);
  });

  it("a cancelled hydrate discards held events and leaves the cursor put", async () => {
    // Dropping is safe precisely because the cursor did not move: the
    // next attempt's tail read fetches those rows again.
    seedWarmSlice(10, "seed");
    const { deps, release } = deferredDeps([row(11, assistantEvent("row-11"))], 11);
    let cancelled = false;
    const running = hydrateThreadByCursor(THREAD, PROVIDER, () => cancelled, deps);
    enqueueAgentChatEvent(THREAD, assistantEvent("row-11"), 11);
    cancelled = true;
    release();
    await running;

    expect(transcript()).toEqual(["seed"]);
    expect(
      useAgentChatStore.getState().threads[THREAD].lastPersistedEventId,
    ).toBe(10);

    // The retry recovers everything the cancelled attempt dropped.
    const retry = deferredDeps([row(11, assistantEvent("row-11"))], 11);
    retry.release();
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, retry.deps);
    expect(transcript()).toEqual(["seed", "row-11"]);
  });

  it("a failed read forgets the cursor so the next visit rebuilds cold", async () => {
    seedWarmSlice(10, "seed");
    const failing: CursorHydrateDeps = {
      listAfter: async () => {
        throw new Error("simulated read failure");
      },
      headId: async () => 11,
      turnActive: async () => false,
    };
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, failing);
    expect(
      useAgentChatStore.getState().threads[THREAD].lastPersistedEventId,
    ).toBeNull();

    // Cold path: one read from the start, transcript replaced wholesale
    // — so the events released after the failure cannot duplicate.
    const cold = deferredDeps(
      [row(10, assistantEvent("seed")), row(11, assistantEvent("row-11"))],
      11,
    );
    cold.release();
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, cold.deps);
    expect(transcript()).toEqual(["seed", "row-11"]);
  });
});

describe("cursor hydrate — path selection", () => {
  beforeEach(() => {
    useAgentChatStore.setState({ threads: {} });
  });

  it("does no reduction when a warm thread has not moved", async () => {
    seedWarmSlice(10, "seed");
    const before = useAgentChatStore.getState().threads[THREAD];
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, {
      listAfter: async () => [],
      headId: async () => 10,
      turnActive: async () => {
        throw new Error("liveness must not be probed for an empty tail");
      },
    });
    expect(useAgentChatStore.getState().threads[THREAD]).toBe(before);
  });

  it("cold-hydrates a thread with no cursor and never probes the head", async () => {
    useAgentChatStore.getState().ensureThread(THREAD);
    const headId = vi.fn<CursorHydrateDeps["headId"]>().mockResolvedValue(2);
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, {
      listAfter: async () => [row(1, assistantEvent("only"))],
      headId,
      turnActive: async () => false,
    });
    expect(headId).not.toHaveBeenCalled();
    expect(transcript()).toEqual(["only"]);
    expect(
      useAgentChatStore.getState().threads[THREAD].lastPersistedEventId,
    ).toBe(1);
  });

  it("cold-hydrates when the cursor sits above the thread's head", async () => {
    // A cursor carried over from a merged/deleted thread: its rows live
    // under another thread_id now, so the tail read looks empty forever.
    seedWarmSlice(900, "stale-seed");
    const listAfter = vi
      .fn<CursorHydrateDeps["listAfter"]>()
      .mockImplementation(async (_thread, afterId) =>
        afterId === null ? [row(5, assistantEvent("real"))] : [],
      );
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, {
      listAfter,
      headId: async () => 5,
      turnActive: async () => false,
    });
    expect(listAfter).toHaveBeenCalledWith(THREAD, 900);
    expect(listAfter).toHaveBeenCalledWith(THREAD, null);
    expect(transcript()).toEqual(["real"]);
    expect(
      useAgentChatStore.getState().threads[THREAD].lastPersistedEventId,
    ).toBe(5);
  });

  it("skips the optimistic bubble a tail row duplicates (matched on nonce)", async () => {
    // Send, then switch away before any provider event: the user_message
    // row lands on disk while the bubble is already rendered locally.
    seedWarmSlice(10, "seed");
    useAgentChatStore.getState().appendUserMessage(THREAD, "do the thing", "nonce-1");
    expect(transcript()).toEqual(["seed", "do the thing"]);

    const rows = [
      userRow(11, "do the thing", "nonce-1"),
      row(12, assistantEvent("on it")),
    ];
    const deps = deferredDeps(rows, 12);
    deps.release();
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, deps.deps);

    expect(transcript()).toEqual(["seed", "do the thing", "on it"]);
  });

  it("promotes a queued bubble when hydrate replays its dispatched user row", async () => {
    seedWarmSlice(10, "seed");
    useAgentChatStore
      .getState()
      .appendUserMessage(THREAD, "queued follow-up", "nonce-queued");
    useAgentChatStore.getState().applyEvent(THREAD, {
      type: "turn_queued",
      thread_id: THREAD,
      queued_id: "q-1",
      client_nonce: "nonce-queued",
      text: "queued follow-up",
    });
    const before = useAgentChatStore.getState().threads[THREAD].messages.find(
      (m) => m.kind === "user_message" && m.clientNonce === "nonce-queued",
    );
    expect(before?.kind === "user_message" ? before.queued : undefined).toEqual({
      queuedId: "q-1",
    });

    // The pane was detached for dispatch, so only the durable user row and
    // later assistant result are available; the dispatch event was lost.
    const deps = deferredDeps(
      [
        userRow(11, "queued follow-up", "nonce-queued"),
        row(12, assistantEvent("handled it", "turn-2")),
      ],
      12,
    );
    deps.release();
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, deps.deps);

    expect(transcript()).toEqual(["seed", "queued follow-up", "handled it"]);
    const after = useAgentChatStore.getState().threads[THREAD].messages.find(
      (m) => m.kind === "user_message" && m.clientNonce === "nonce-queued",
    );
    expect(
      after?.kind === "user_message" ? after.queued : undefined,
    ).toBeUndefined();
  });

  it("applies a tail user_message that has no local bubble", async () => {
    // Same row, no optimistic bubble (another window sent it, or this
    // pane never saw the send) — it must render.
    seedWarmSlice(10, "seed");
    const deps = deferredDeps([userRow(11, "from elsewhere", "nonce-9")], 11);
    deps.release();
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, deps.deps);
    expect(transcript()).toEqual(["seed", "from elsewhere"]);
  });
});

describe("cursor hydrate — an empty cold read must not wipe a live transcript", () => {
  beforeEach(() => {
    useAgentChatStore.setState({ threads: {} });
  });

  /** The resume sequence: the picked chat's history is painted under a
   *  BRAND-NEW thread id, and the backend re-homes those rows onto it
   *  afterwards. A hydrate landing inside that window reads zero rows for
   *  the new id — which used to replace the transcript with `[]`. */
  function seedResumedSlice() {
    useAgentChatStore
      .getState()
      .hydrateThread(THREAD, [
        row(41, assistantEvent("resumed history", "turn-0")),
      ]);
  }

  it("keeps the transcript, drops the cursor, and recovers on the retry", async () => {
    seedResumedSlice();
    const listAfter = vi
      .fn<CursorHydrateDeps["listAfter"]>()
      // Pass 1: rows not re-homed yet — the tail AND the full read are
      // both empty for this thread id.
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([])
      // Pass 2 (after the retry backoff): the re-home committed.
      .mockResolvedValue([row(41, assistantEvent("resumed history", "turn-0"))]);

    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, {
      listAfter,
      // No rows under this id yet, so the head probe finds nothing and the
      // cursor looks "ahead" — the cold path.
      headId: async () => null,
      turnActive: async () => false,
      delay: async () => undefined,
    });

    // The transcript the user is looking at survived both passes.
    expect(transcript()).toEqual(["resumed history"]);
    expect(
      useAgentChatStore.getState().threads[THREAD].lastPersistedEventId,
    ).toBe(41);
  });

  it("leaves the cursor null when even the retry reads nothing", async () => {
    seedResumedSlice();
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, {
      listAfter: async () => [],
      headId: async () => null,
      turnActive: async () => false,
      delay: async () => undefined,
    });
    // Nothing wiped, and the invalidated cursor means the next visit
    // rebuilds cold rather than trusting a position it never confirmed.
    expect(transcript()).toEqual(["resumed history"]);
    expect(
      useAgentChatStore.getState().threads[THREAD].lastPersistedEventId,
    ).toBeNull();
  });

  it("still hydrates an genuinely empty thread that has no local messages", async () => {
    // The guard is about protecting RENDERED messages — a fresh slice has
    // none, so the empty hydrate proceeds and syncs the cursor to 0.
    useAgentChatStore.getState().ensureThread(THREAD);
    const listAfter = vi
      .fn<CursorHydrateDeps["listAfter"]>()
      .mockResolvedValue([]);
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, {
      listAfter,
      headId: async () => null,
      turnActive: async () => false,
      delay: async () => undefined,
    });
    expect(
      useAgentChatStore.getState().threads[THREAD].lastPersistedEventId,
    ).toBe(0);
    // One pass only — nothing to protect means nothing to retry.
    expect(listAfter).toHaveBeenCalledTimes(1);
  });
});

describe("cursor hydrate — held deltas settled by the read", () => {
  beforeEach(() => {
    useAgentChatStore.setState({ threads: {} });
  });

  function textDelta(text: string, turnId = "turn-1"): ProviderRuntimeEvent {
    return {
      type: "content_delta",
      thread_id: THREAD,
      turn_id: turnId,
      delta: { kind: "text", text },
    };
  }

  function thinkingDelta(text: string, turnId = "turn-1"): ProviderRuntimeEvent {
    return {
      type: "content_delta",
      thread_id: THREAD,
      turn_id: turnId,
      delta: { kind: "thinking", text },
    };
  }

  it("does not duplicate an assistant block the tail already settled", async () => {
    // Mid-turn remount: deltas stream into the held buffer, then the
    // `item_completed` that settles them arrives BOTH live and in the
    // tail. Replaying the deltas after the tail applied the finished block
    // would mint a second bubble with the same words.
    seedWarmSlice(10, "seed");
    const settled = assistantEvent("hello world", "turn-1");
    const { deps, release } = deferredDeps([row(11, settled)], 11);
    const running = hydrateThreadByCursor(THREAD, PROVIDER, () => false, deps);

    enqueueAgentChatEvent(THREAD, textDelta("hello "));
    enqueueAgentChatEvent(THREAD, textDelta("world"));
    enqueueAgentChatEvent(THREAD, settled, 11);

    release();
    await running;
    flushAllAgentChatEvents();

    expect(transcript()).toEqual(["seed", "hello world"]);
  });

  it("does not duplicate a reasoning block the tail already settled", async () => {
    seedWarmSlice(10, "seed");
    const settled = {
      type: "item_completed",
      thread_id: THREAD,
      turn_id: "turn-1",
      item: { kind: "assistant_thinking", text: "a whole thought" },
    } as ProviderRuntimeEvent;
    const { deps, release } = deferredDeps([row(11, settled)], 11);
    const running = hydrateThreadByCursor(THREAD, PROVIDER, () => false, deps);

    enqueueAgentChatEvent(THREAD, thinkingDelta("a whole "));
    enqueueAgentChatEvent(THREAD, thinkingDelta("thought"));
    enqueueAgentChatEvent(THREAD, settled, 11);

    release();
    await running;
    flushAllAgentChatEvents();

    const reasoning = useAgentChatStore
      .getState()
      .threads[THREAD].messages.filter((m) => m.kind === "reasoning");
    expect(reasoning).toHaveLength(1);
    expect(reasoning[0].kind === "reasoning" && reasoning[0].text).toBe(
      "a whole thought",
    );
  });

  it("keeps deltas that are NEWER than everything the read returned", async () => {
    // The next block of the same turn streams during the hold window and
    // has no settled row behind it — dropping it would be a gap.
    seedWarmSlice(10, "seed");
    const settled = assistantEvent("first block", "turn-1");
    const { deps, release } = deferredDeps([row(11, settled)], 11);
    const running = hydrateThreadByCursor(THREAD, PROVIDER, () => false, deps);

    enqueueAgentChatEvent(THREAD, settled, 11);
    enqueueAgentChatEvent(THREAD, textDelta("second block", "turn-1"));

    release();
    await running;
    flushAllAgentChatEvents();

    expect(transcript()).toEqual(["seed", "first block", "second block"]);
  });

  it("keeps every delta when the read settled nothing the buffer saw", async () => {
    seedWarmSlice(10, "seed");
    const { deps, release } = deferredDeps([row(11, assistantEvent("row-11"))], 11);
    const running = hydrateThreadByCursor(THREAD, PROVIDER, () => false, deps);
    enqueueAgentChatEvent(THREAD, textDelta("live tokens", "turn-9"));
    release();
    await running;
    flushAllAgentChatEvents();

    expect(transcript()).toEqual(["seed", "row-11", "live tokens"]);
  });
});

describe("cursor hydrate — overlapping hydrates (hold ownership)", () => {
  beforeEach(() => {
    useAgentChatStore.setState({ threads: {} });
  });

  it("a stale hydrate's teardown does not disturb the newer one", async () => {
    // Unmount/remount inside the IPC window. #1 is cancelled and drops;
    // that drop must not unpark the thread for #2, whose tail read is
    // still in flight — live events applied there would be double-applied
    // by #2's merge.
    seedWarmSlice(10, "seed");
    const first = deferredDeps([row(11, assistantEvent("row-11"))], 11);
    let cancelledFirst = false;
    const running1 = hydrateThreadByCursor(
      THREAD,
      PROVIDER,
      () => cancelledFirst,
      first.deps,
    );

    const second = deferredDeps([row(11, assistantEvent("row-11"))], 11);
    const running2 = hydrateThreadByCursor(
      THREAD,
      PROVIDER,
      () => false,
      second.deps,
    );

    // The row also arrives live while #2 reads.
    enqueueAgentChatEvent(THREAD, assistantEvent("row-11"), 11);

    cancelledFirst = true;
    first.release();
    await running1;
    // #1's drop was a no-op: #2 still owns the hold, so nothing applied.
    expect(transcript()).toEqual(["seed"]);

    second.release();
    await running2;
    flushAllAgentChatEvents();

    expect(transcript()).toEqual(["seed", "row-11"]);
    expect(
      useAgentChatStore.getState().threads[THREAD].lastPersistedEventId,
    ).toBe(11);
  });

  it("a double-run of the same hydrate converges (dev double-mount)", async () => {
    seedWarmSlice(10, "seed");
    const rows = [row(11, assistantEvent("row-11")), row(12, assistantEvent("row-12"))];
    const deps: CursorHydrateDeps = {
      listAfter: async (_thread, afterId) =>
        afterId === null ? rows : rows.filter((r) => r.id > afterId),
      headId: async () => 12,
      turnActive: async () => false,
    };

    await Promise.all([
      hydrateThreadByCursor(THREAD, PROVIDER, () => false, deps),
      hydrateThreadByCursor(THREAD, PROVIDER, () => false, deps),
    ]);
    flushAllAgentChatEvents();

    expect(transcript()).toEqual(["seed", "row-11", "row-12"]);
    expect(
      useAgentChatStore.getState().threads[THREAD].lastPersistedEventId,
    ).toBe(12);
  });
});

describe("cursor hydrate — read-after-attach", () => {
  beforeEach(() => {
    useAgentChatStore.setState({ threads: {} });
  });

  it("waits for the attach before reading, so a row persisted in that window is not skipped", async () => {
    // The backend drops events for a thread with no attached channel. A
    // read that preceded the attach would miss row 11 forever: the next
    // live event (12) advances the cursor straight past it.
    seedWarmSlice(10, "seed");
    let attachDone!: () => void;
    const attached = new Promise<void>((resolve) => {
      attachDone = resolve;
    });
    const listAfter = vi
      .fn<CursorHydrateDeps["listAfter"]>()
      .mockImplementation(async (_thread, afterId) =>
        [row(11, assistantEvent("persisted-during-attach"))].filter(
          (r) => afterId === null || r.id > afterId,
        ),
      );

    const running = hydrateThreadByCursor(THREAD, PROVIDER, () => false, {
      listAfter,
      headId: async () => 11,
      turnActive: async () => false,
      awaitAttach: () => attached,
    });
    await Promise.resolve();
    // Nothing read yet — the channel is not live, so a read now could not
    // be backed up by the held buffer.
    expect(listAfter).not.toHaveBeenCalled();

    attachDone();
    await running;
    // Row 12 arrives live, on the now-attached channel.
    enqueueAgentChatEvent(THREAD, assistantEvent("live-12"), 12);
    flushAllAgentChatEvents();

    expect(transcript()).toEqual([
      "seed",
      "persisted-during-attach",
      "live-12",
    ]);
    expect(
      useAgentChatStore.getState().threads[THREAD].lastPersistedEventId,
    ).toBe(12);
  });

  it("reads anyway when the attach never resolves (timeout fallback)", async () => {
    seedWarmSlice(10, "seed");
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, {
      listAfter: async () => [row(11, assistantEvent("row-11"))],
      headId: async () => 11,
      turnActive: async () => false,
      // Stands in for the registry's timeout arm winning the race.
      awaitAttach: async () => undefined,
    });
    expect(transcript()).toEqual(["seed", "row-11"]);
  });
});

/**
 * The liveness probe's failure mode.
 *
 * `agent_chat_turn_active` rejects for reasons unrelated to liveness — the
 * feature flag being off, a `provider_not_configured` registry miss during
 * startup. The old `.catch(() => false)` turned every one of those into
 * "the run is dead", which is the single answer that flips a healthy
 * transcript to "Run interrupted" with a Continue chip.
 */
describe("cursor hydrate — the turn-active probe", () => {
  beforeEach(() => {
    useAgentChatStore.setState({ threads: {} });
  });

  /** A warm slice mid-run: streaming, with its last turn settled by an
   *  interim `turn_completed` (the provider yielded on delegated work). */
  function seedStreamingSlice(cursor: number) {
    const store = useAgentChatStore.getState();
    store.ensureThread(THREAD);
    store.applyEvent(THREAD, {
      type: "user_message",
      thread_id: THREAD,
      text: "go",
    } as ProviderRuntimeEvent);
    store.applyEvent(THREAD, {
      type: "session_state_changed",
      thread_id: THREAD,
      status: { status: "running" },
    } as ProviderRuntimeEvent);
    useAgentChatStore.setState((state) => ({
      threads: {
        ...state.threads,
        [THREAD]: { ...state.threads[THREAD], lastPersistedEventId: cursor },
      },
    }));
    expect(useAgentChatStore.getState().threads[THREAD].streaming).toBe(true);
  }

  it("a rejected probe does not mark a live thread interrupted", async () => {
    seedStreamingSlice(10);
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, {
      listAfter: async (_t, afterId) =>
        afterId === null ? [] : [row(11, assistantEvent("still working"))],
      headId: async () => 11,
      turnActive: async () => {
        throw new Error("provider_not_configured: Claude");
      },
    });
    const slice = useAgentChatStore.getState().threads[THREAD];
    expect(slice.interrupted).toBe(false);
    expect(slice.streaming).toBe(true);
  });

  it("a definite false still settles a thread that is not streaming", async () => {
    // The probe answering "dead" is honoured — only an UNANSWERED probe
    // falls back to the slice. Otherwise a genuinely dead run could never
    // earn its divider.
    seedStreamingSlice(10);
    useAgentChatStore.setState((state) => ({
      threads: {
        ...state.threads,
        [THREAD]: { ...state.threads[THREAD], streaming: false },
      },
    }));
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, {
      listAfter: async (_t, afterId) =>
        afterId === null ? [] : [row(11, assistantEvent("last words"))],
      headId: async () => 11,
      turnActive: async () => false,
    });
    const slice = useAgentChatStore.getState().threads[THREAD];
    expect(slice.streaming).toBe(false);
    expect(slice.interrupted).toBe(true);
  });

  it("bounds the no-information fallback so a permanently broken probe settles", async () => {
    // The fallback is a fixpoint if it is unbounded: hydrate writes
    // `streaming: runLive`, so a streaming slice re-derives `true` from
    // itself on every hydrate and can never settle while the probe keeps
    // rejecting — which it does for as long as the flag stays off or the
    // provider stays out of the registry. Own thread id: the fallback
    // counter is per-thread, and this drives several hydrates over one.
    const THREAD_2 = "thread-probe-fixpoint";
    const store = useAgentChatStore.getState();
    store.ensureThread(THREAD_2);
    store.applyEvent(THREAD_2, {
      type: "user_message",
      thread_id: THREAD_2,
      text: "go",
    } as ProviderRuntimeEvent);
    store.applyEvent(THREAD_2, {
      type: "session_state_changed",
      thread_id: THREAD_2,
      status: { status: "running" },
    } as ProviderRuntimeEvent);

    const hydrateOnce = async (cursor: number) => {
      useAgentChatStore.setState((state) => ({
        threads: {
          ...state.threads,
          [THREAD_2]: { ...state.threads[THREAD_2], lastPersistedEventId: cursor },
        },
      }));
      await hydrateThreadByCursor(THREAD_2, PROVIDER, () => false, {
        listAfter: async (_t, afterId) =>
          afterId === null ? [] : [row(cursor + 1, assistantEvent("tick"))],
        headId: async () => cursor + 1,
        turnActive: async () => {
          throw new Error("provider_not_configured: Claude");
        },
      });
      return useAgentChatStore.getState().threads[THREAD_2];
    };

    // The first few unanswered probes still defer to the slice — that is
    // the startup window the "no information" rule exists for.
    expect((await hydrateOnce(10)).streaming).toBe(true);
    expect((await hydrateOnce(11)).streaming).toBe(true);
    expect((await hydrateOnce(12)).streaming).toBe(true);
    // Past the bound, "no information" settles instead of carrying a
    // stuck spinner (and a suppressed Continue chip) for the session.
    expect((await hydrateOnce(13)).streaming).toBe(false);
  });

  it("a probe reporting a delegated-work hold keeps the run live", async () => {
    // The backend fix: `agent_chat_turn_active` now ORs in "the parent turn
    // settled but delegated agents are still working", so the probe answers
    // true across the whole delegated phase.
    seedStreamingSlice(10);
    await hydrateThreadByCursor(THREAD, PROVIDER, () => false, {
      listAfter: async (_t, afterId) =>
        afterId === null ? [] : [row(11, assistantEvent("subagent output"))],
      headId: async () => 11,
      turnActive: async () => true,
    });
    const slice = useAgentChatStore.getState().threads[THREAD];
    expect(slice.interrupted).toBe(false);
    expect(slice.streaming).toBe(true);
  });
});
