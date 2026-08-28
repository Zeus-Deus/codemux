import type { ProviderRuntimeEvent } from "@/tauri/events";
import type { AgentChatProviderKind } from "@/tauri/types";

import { providerRequestsSurviveSessionRestart } from "./capability-defaults";
import {
  applyEvent,
  createEmptyThreadState,
} from "./reducer";
import { interruptRunningSubagents } from "./subagents";
import type { ChatThreadState, ChatViewItem } from "./types";

/**
 * Synthetic envelope written by the backend to record user turns.
 * Providers never echo user messages back on the event stream, so they
 * are persisted under this tag to keep everything that affects the
 * rendered transcript inside one ordered list.
 *
 * It is no longer replay-only: the backend also fans the freshly-written
 * row out to every client attached to the thread, under this exact shape,
 * so a second viewer sees the turn live AND advances its cursor over the
 * row. That is why the tag now lives in the `ProviderRuntimeEvent` union
 * (`src/tauri/events.ts`) with one reducer case serving both paths, and
 * this alias just names the variant for the replay code.
 */
export type UserMessageEnvelope = Extract<
  ProviderRuntimeEvent,
  { type: "user_message" }
>;

export type ReplayPayload = ProviderRuntimeEvent;

/** Parsed history row with the SQLite insertion time that owned it. The
 *  timestamp is optional for compatibility with older callers/tests. */
export interface TimedReplayPayload {
  event: ReplayPayload;
  createdAtMs?: number;
  persistedId?: number;
}

const STALE_REQUEST_MESSAGE =
  "This request expired when the provider session restarted. Continue to have the agent repeat it.";

/**
 * Gap ceiling for warm resume: past this many unapplied rows, rebuilding
 * the thread cold is both cheaper and simpler than merging a tail (the
 * merge holds live events for its duration, and a tail that large means
 * the pane was away for a whole session's worth of work).
 */
export const MAX_WARM_TAIL_ROWS = 2_000;

/** Options accepted by {@link replayPayloads} (and forwarded verbatim by
 *  `agent-chat-store.hydrateThread`). */
export interface ReplayOptions {
  /** The backend authoritatively confirmed a turn is in flight. */
  runLive?: boolean;
  /** Provider that owns the thread. Decides whether an unresolved
   *  persisted request is still answerable once no turn is live — see
   *  `providerRequestsSurviveSessionRestart`. Omitted means "assume
   *  process-local callbacks", i.e. today's expire-on-hydrate behavior. */
  provider?: AgentChatProviderKind | null;
}

/**
 * Rebuild a thread state by replaying persisted payloads through the
 * same pure reducer that handles live events.
 *
 * Each input is a JSON string — typically the rows returned by
 * `agent_chat_list_messages`. Malformed JSON or unknown envelope
 * shapes are silently skipped so a single bad row can never wedge
 * the entire transcript.
 *
 * The returned state has `streaming: false` regardless of whether
 * the original session ended cleanly. The caller is on the resume
 * path; a fresh provider session will flip streaming back on once
 * the user sends the next turn.
 *
 * `opts.runLive` overrides that resting default for one specific
 * false-positive: a live run whose pane was unmounted and remounted
 * (switching away from and back to a workspace tears the whole inactive
 * workspace's pane tree down, so the live event listener detaches while
 * the backend keeps streaming and persisting rows). On remount the
 * persisted tail has no terminal event after the last user turn, so the
 * `lastTurnUnsettled` heuristic would flag the transcript as
 * "interrupted" and render a Run-interrupted divider over a perfectly
 * healthy run. When the caller has authoritatively confirmed a turn is
 * in flight (`runLive: true`), we force `interrupted: false` — suppressing
 * BOTH the `lastTurnUnsettled` heuristic AND any replay-observed
 * `child_exited` flag, since an in-flight turn means the run recovered /
 * is alive — and set `streaming: true` so the streaming marker shows
 * instead of the divider. The next live event keeps or clears that flag
 * through the reducer as usual. With `opts` absent (or `runLive` falsy)
 * behavior is identical to the resume path.
 *
 * `opts.provider` gates the synthetic expiry of unresolved requests: a
 * provider whose request state lives outside the Codemux process (OpenCode)
 * keeps its pending requests actionable even with no live turn.
 */
export function replayPayloads(
  payloads: string[],
  opts?: ReplayOptions,
): ChatThreadState {
  return replayParsed(parseReplayPayloads(payloads), opts);
}

/**
 * {@link replayPayloads} over already-parsed rows. The cursor hydrate
 * path parses each payload exactly once and then feeds both the fold and
 * the tail heuristics from the same array — the old code re-parsed every
 * row up to four times per mount.
 */
export function replayParsed(
  parsed: ReplayPayload[],
  opts?: ReplayOptions,
): ChatThreadState {
  return replayTimed(
    parsed.map((event) => ({ event })),
    opts,
  );
}

/** Replay rows using their durable insertion times so restored turn durations
 *  are identical to the live transcript instead of being stamped at mount. */
export function replayTimed(
  timed: TimedReplayPayload[],
  opts?: ReplayOptions,
): ChatThreadState {
  const parsed = timed.map((row) => row.event);
  const state = foldReplayPayloads(createEmptyThreadState(), timed);
  return finalizeReplay(state, parsed, opts, false, false);
}

/**
 * Merge a CURSOR TAIL (rows the slice has not applied yet) into a warm
 * thread state, in one ordered reduction.
 *
 * This is the warm-revisit counterpart of {@link replayParsed}: the
 * transcript prefix is already in memory, so only the rows after the
 * slice's `lastPersistedEventId` are folded — but the post-replay
 * heuristics still have to run over the MERGED result, because every one
 * of them (orphan-request expiry, reasoning unseal, subagent interrupt,
 * workflow stop, the unsettled-tail flag) is a property of the whole
 * history, not of the tail.
 *
 * `previousUnsettled` summarizes the prefix for the unsettled-tail scan —
 * the caller passes the slice's tracked `turnUnsettled`. Any `user_message`
 * / `turn_completed` in the tail overrides it, which makes the merged
 * answer identical to scanning the concatenated history.
 *
 * When the run is live the settle passes are skipped entirely: their job
 * is to reconcile a transcript that ends mid-run, and a confirmed
 * in-flight turn means those spinners are real.
 */
export function applyReplayTail(
  base: ChatThreadState,
  parsed: ReplayPayload[],
  opts: ReplayOptions & { previousUnsettled: boolean },
): ChatThreadState {
  return applyTimedReplayTail(
    base,
    parsed.map((event) => ({ event })),
    opts,
  );
}

export function applyTimedReplayTail(
  base: ChatThreadState,
  timed: TimedReplayPayload[],
  opts: ReplayOptions & { previousUnsettled: boolean },
): ChatThreadState {
  const parsed = timed.map((row) => row.event);
  const merged = foldReplayPayloads(base, timed);
  return finalizeReplay(
    merged,
    parsed,
    opts,
    opts.previousUnsettled,
    opts.runLive ?? false,
  );
}

/** Fold parsed rows through the same reducer the live stream uses.
 *
 *  Every row kind, `user_message` included, goes through `applyEvent`:
 *  the backend now fans a persisted user turn out live under the same
 *  `{"type":"user_message"}` shape it stores, so replaying a row and
 *  receiving one must not be able to disagree. The nonce dedup and the
 *  `images` path→`src` mapping live in the reducer's case. */
function foldReplayPayloads(
  base: ChatThreadState,
  timed: TimedReplayPayload[],
): ChatThreadState {
  let state = base;
  for (const row of timed) {
    const previous = state;
    state = applyEvent(
      state,
      row.event,
      row.createdAtMs == null ? undefined : () => row.createdAtMs!,
    );
    if (row.persistedId != null) {
      state = stampSearchSourceId(previous, state, row.event, row.persistedId);
    }
  }
  return state;
}

/** Attach a durable search-row id to the visible prose item produced by one
 * persisted event. The live path uses this too: a user bubble may already
 * exist optimistically, so matching by nonce/text is as important as finding
 * a newly-appended item. */
export function stampSearchSourceId<T extends ChatThreadState>(
  previous: ChatThreadState,
  next: T,
  event: ReplayPayload,
  persistedId: number,
): T {
  let targetIndex = -1;
  if (event.type === "user_message") {
    for (let i = next.messages.length - 1; i >= 0; i--) {
      const item = next.messages[i];
      if (
        item.kind === "user_message" &&
        (event.client_nonce
          ? item.clientNonce === event.client_nonce
          : item.text === event.text &&
            (i >= previous.messages.length || previous.messages[i] !== item))
      ) {
        targetIndex = i;
        break;
      }
    }
  } else if (
    event.type === "item_completed" &&
    !event.subagent_id &&
    event.item.kind === "assistant_text"
  ) {
    for (let i = next.messages.length - 1; i >= 0; i--) {
      const item = next.messages[i];
      if (
        item.kind === "assistant_message" &&
        item.turn_id === event.turn_id &&
        item.text === event.item.text
      ) {
        targetIndex = i;
        break;
      }
    }
  }
  if (targetIndex < 0) return next;
  const target = next.messages[targetIndex];
  if (
    (target.kind !== "user_message" && target.kind !== "assistant_message") ||
    target.source_event_id === persistedId
  ) {
    return next;
  }
  const messages = next.messages.slice();
  messages[targetIndex] = { ...target, source_event_id: persistedId };
  return { ...next, messages };
}

/**
 * The post-replay passes, shared by cold replay and tail merge. Operates
 * on the FULL (merged) state; `parsed` is only read for the tail
 * heuristics.
 */
function finalizeReplay(
  input: ChatThreadState,
  parsed: ReplayPayload[],
  opts: ReplayOptions | undefined,
  previousUnsettled: boolean,
  skipSettlePasses: boolean,
): ChatThreadState {
  const runLive = opts?.runLive ?? false;
  // Only providers with process-local callbacks lose their requests when the
  // session dies. OpenCode's permissions live in its HTTP server, and
  // `agent_chat_respond_to_request` re-adopts that session to deliver the
  // answer, so expiring them here would render a still-answerable request
  // dead — and the reducer's `request_opened` early-return means no
  // re-broadcast could ever bring it back.
  const requestsSurvive = providerRequestsSurviveSessionRestart(opts?.provider);
  const expireOrphanRequests = !runLive && !requestsSurvive;
  let state = input;

  // Provider request callbacks are ephemeral even though the transcript is
  // durable. When no live turn owns the replayed requests, settle every
  // orphan through the same reducer event used by a failed live response.
  // This is the part the old `pendingRequestIds: []` cleanup missed: the
  // rendered PermissionRequestItem itself stayed `pending`, so the composer
  // resurrected the questionnaire despite the supposedly-empty pending set.
  //
  // A workspace-switch remount can happen while the original provider turn
  // is still alive. `runLive` is the backend's authoritative guard for that
  // case, so those callbacks remain actionable.
  if (expireOrphanRequests) {
    for (const requestId of [...state.pendingRequestIds]) {
      state = applyEvent(state, {
        type: "request_response_failed",
        thread_id: "",
        request_id: requestId,
        reason: "stale_provider_callback",
        message: STALE_REQUEST_MESSAGE,
      });
    }
  }
  // Guarantee post-replay state matches a quiescent thread. The
  // reducer's turn_completed handler already clears `streaming`, but
  // not every transcript ends with one (interrupted resume, partial
  // history, …). Belt and braces.
  //
  // A transcript truncated mid-thinking (no `assistant_thinking`
  // completion, no trailing boundary item) can leave a reasoning block
  // flagged `streaming`. Seal those so a restored thread never renders a
  // perpetual "Thinking…" shimmer.
  //
  // Skipped on the same condition as the other settle passes: a tail
  // merged into a CONFIRMED-live run is sealing a block the provider is
  // still streaming into. The next thinking delta would find no streaming
  // reasoning tail and mint a second block — the shimmer is real here.
  const hasStreamingReasoning =
    !skipSettlePasses &&
    state.messages.some((m) => m.kind === "reasoning" && m.streaming);
  let messages: ChatViewItem[] = hasStreamingReasoning
    ? state.messages.map((m) =>
        m.kind === "reasoning" && m.streaming ? { ...m, streaming: false } : m,
      )
    : state.messages;

  // Belt-and-braces run-state reconciliation (issue #153): a transcript
  // that ends mid-run — the last persisted event was a `running` snapshot
  // with no terminal snapshot after it — must NEVER hydrate with a live
  // spinner. Interrupt every still-running subagent (in cards and
  // workflow phases) and stop any still-`running` workflow. This mirrors
  // the reducer's own settle paths and self-heals: if the persisted
  // stream actually did settle a subagent (e.g. a parent `tool_result`
  // the reducer derived from), that terminal state already won during
  // replay and is left untouched here.
  //
  // A `pending_approval` workflow whose request is still answerable (live
  // turn, or a provider whose requests survive the session) is deliberately
  // preserved. A genuinely-orphaned one was already stopped above by
  // `request_response_failed`, since resuming conversation history cannot
  // recreate a process-local provider callback.
  //
  // Skipped for a tail merged into a confirmed-live run: the warm slice
  // already holds live cards whose spinners belong to the turn that is
  // still executing. (A cold replay always runs them — it is rebuilding
  // the transcript from persisted rows alone, where a `running` snapshot
  // with no terminal successor genuinely means "never settled".)
  if (!skipSettlePasses) {
    messages = interruptRunningSubagents(messages);
    messages = messages.map((m) =>
      m.kind === "workflow_run" && m.status === "running"
        ? { ...m, status: "stopped" as const }
        : m,
    );
  }

  const turnUnsettled = lastTurnUnsettled(parsed, previousUnsettled);

  return {
    ...state,
    messages,
    // Normally a resting resume state, but when the backend confirms a
    // turn is in flight we mark the thread streaming so the streaming
    // marker shows instead of the Run-interrupted divider.
    streaming: runLive,
    pendingRequestIds: expireOrphanRequests ? [] : state.pendingRequestIds,
    // Interrupted when EITHER the replay itself observed a `child_exited`
    // terminal turn (the watchdog persisted a `turn_completed` error) OR
    // the raw history ends with an unsettled user turn (the app died before
    // any terminal event was written — laptop sleep, hard crash). Both mean
    // "the last run never cleanly finished", so the tail shows the "Run
    // interrupted" divider and the composer offers a Continue chip.
    //
    // `runLive` overrides both signals: a confirmed in-flight turn means
    // the run is alive, so neither the unsettled-tail heuristic nor a
    // stale replay-observed `child_exited` should surface the divider.
    interrupted: runLive ? false : state.interrupted || turnUnsettled,
    // Recorded raw — WITHOUT the `runLive` override — because this is the
    // seed the next cursor-tail merge scans from, and it has to describe the
    // history, not the current liveness verdict. Folding `runLive` in here
    // would make a tail read taken while a turn happened to be live disagree
    // with a full-history scan of the same rows.
    turnUnsettled,
    // Transient — never persisted, so a hydrated thread always starts clear.
    stalled: null,
  };
}

/**
 * Pure helper: does the persisted history end with an unsettled user turn?
 *
 * Scans the ordered rows for the LAST `user_message` envelope and returns
 * true iff one exists and NO later row is a `turn_completed` event.
 * `turn_completed` is the sole settlement marker. A persisted
 * `session_state_changed{Error}` is NOT a second one: the watchdog emits
 * a persisted `turn_completed` before it, so any turn an error settles
 * already carries its own marker and scanning for errors here would only
 * duplicate that. Queued-turn envelopes persist at dispatch time, so
 * ordering is sound. Exported for direct unit testing.
 *
 * Takes ALREADY-PARSED rows: the hydrate path parses each payload once
 * and shares the array with the fold. `previous` seeds the scan with the
 * answer for the history before `parsed` (always `false` for a cold
 * replay, the warm slice's state for a cursor tail), which makes a tail
 * scan equal to scanning the whole concatenated history.
 */
export function lastTurnUnsettled(
  parsed: ReplayPayload[],
  previous = false,
): boolean {
  let unsettled = previous;
  for (const row of parsed) {
    if (row.type === "user_message") unsettled = true;
    else if (row.type === "turn_completed") unsettled = false;
  }
  return unsettled;
}

/** Parse raw payload strings once, dropping rows that are not usable
 *  envelopes (malformed JSON, unknown shapes). */
export function parseReplayPayloads(payloads: string[]): ReplayPayload[] {
  const parsed: ReplayPayload[] = [];
  for (const raw of payloads) {
    const row = parsePayload(raw);
    if (row) parsed.push(row);
  }
  return parsed;
}

/** Parse cursor/history rows while preserving their durable timestamps. */
export function parseTimedReplayPayloads(
  rows: ReadonlyArray<{ id?: number; payload: string; created_at_ms?: number }>,
): TimedReplayPayload[] {
  const parsed: TimedReplayPayload[] = [];
  for (const source of rows) {
    const event = parsePayload(source.payload);
    if (!event) continue;
    const createdAtMs = source.created_at_ms;
    parsed.push({
      event,
      ...(typeof source.id === "number" && Number.isFinite(source.id)
        ? { persistedId: source.id }
        : {}),
      ...(typeof createdAtMs === "number" && Number.isFinite(createdAtMs)
        ? { createdAtMs }
        : {}),
    });
  }
  return parsed;
}

function parsePayload(raw: string): ReplayPayload | null {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }
  if (
    !value ||
    typeof value !== "object" ||
    typeof (value as { type?: unknown }).type !== "string"
  ) {
    return null;
  }
  const type = (value as { type: string }).type;
  if (type === "user_message") {
    const text = (value as { text?: unknown }).text;
    if (typeof text !== "string") return null;
    // Sanitize the optional `images` array: keep only well-formed
    // entries with a string `path`, so a malformed row can never feed
    // a non-string `src` into the render layer. A missing / non-array
    // `images` field yields `undefined` (text-only turn).
    const rawImages = (value as { images?: unknown }).images;
    const images = Array.isArray(rawImages)
      ? rawImages.flatMap((entry) => {
          if (!entry || typeof entry !== "object") return [];
          const path = (entry as { path?: unknown }).path;
          if (typeof path !== "string") return [];
          const mediaType = (entry as { media_type?: unknown }).media_type;
          return [
            {
              path,
              media_type: typeof mediaType === "string" ? mediaType : undefined,
            },
          ];
        })
      : undefined;
    const nonce = (value as { client_nonce?: unknown }).client_nonce;
    return {
      type: "user_message",
      thread_id: (value as { thread_id?: string }).thread_id ?? "",
      text,
      images,
      client_nonce: typeof nonce === "string" && nonce ? nonce : undefined,
    };
  }
  // Provider events — trust the discriminated union shape. Unknown
  // discriminators fall through to the reducer's `warnOnce` path.
  return value as ProviderRuntimeEvent;
}
