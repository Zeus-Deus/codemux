import type { ProviderRuntimeEvent } from "@/tauri/events";

import {
  applyEvent,
  appendUserMessage,
  createEmptyThreadState,
} from "./reducer";
import { interruptRunningSubagents } from "./subagents";
import type { ChatThreadState, ChatViewItem, UserMessageImage } from "./types";

/**
 * Synthetic envelope written by the backend in `agent_chat_send_turn`
 * to record user turns. User messages never come back through the
 * provider event stream, so we persist them under this tag to keep
 * everything that affects the rendered transcript inside a single
 * ordered list. The frontend reducer has no notion of this envelope
 * — `replayPayloads` translates it into `appendUserMessage`.
 */
export interface UserMessageEnvelope {
  type: "user_message";
  thread_id: string;
  text: string;
  /** Images attached to this turn, written by the backend when the
   *  turn carried paste/drop/picker images. Each `path` is an absolute
   *  filesystem location the webview loads via Tauri's asset protocol
   *  (mapped onto the bubble item's `images[].src`). Absent for
   *  text-only turns and for turns persisted before the field existed. */
  images?: Array<{ path: string; media_type?: string }>;
}

type ReplayPayload = UserMessageEnvelope | ProviderRuntimeEvent;

const STALE_REQUEST_MESSAGE =
  "This request expired when the provider session restarted. Continue to have the agent repeat it.";

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
 */
export function replayPayloads(
  payloads: string[],
  opts?: { runLive?: boolean },
): ChatThreadState {
  const runLive = opts?.runLive ?? false;
  let state = createEmptyThreadState();
  for (const raw of payloads) {
    const parsed = parsePayload(raw);
    if (!parsed) continue;
    if (parsed.type === "user_message") {
      // Map the persisted `images` (absolute paths) onto the bubble's
      // display shape; `src` is the path, which `resolveAssetSrc`
      // routes through Tauri's asset protocol at render time.
      const images: UserMessageImage[] | undefined = parsed.images?.map(
        (img) => ({ src: img.path, mediaType: img.media_type }),
      );
      state = appendUserMessage(state, parsed.text, undefined, undefined, images);
    } else {
      state = applyEvent(state, parsed);
    }
  }

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
  if (!runLive) {
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
  const hasStreamingReasoning = state.messages.some(
    (m) => m.kind === "reasoning" && m.streaming,
  );
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
  // A live `pending_approval` workflow is deliberately preserved. A replayed
  // orphan was already stopped above by `request_response_failed`; resuming
  // conversation history cannot recreate its provider callback.
  messages = interruptRunningSubagents(messages);
  messages = messages.map((m) =>
    m.kind === "workflow_run" && m.status === "running"
      ? { ...m, status: "stopped" as const }
      : m,
  );

  return {
    ...state,
    messages,
    // Normally a resting resume state, but when the backend confirms a
    // turn is in flight we mark the thread streaming so the streaming
    // marker shows instead of the Run-interrupted divider.
    streaming: runLive,
    pendingRequestIds: runLive ? state.pendingRequestIds : [],
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
    interrupted: runLive
      ? false
      : state.interrupted || lastTurnUnsettled(payloads),
    // Transient — never persisted, so a hydrated thread always starts clear.
    stalled: null,
  };
}

/**
 * Pure helper: does the persisted history end with an unsettled user turn?
 *
 * Scans the ordered raw payloads for the LAST `user_message` envelope and
 * returns true iff one exists and NO later payload is a `turn_completed`
 * event. `session_state_changed` is never persisted, so `turn_completed`
 * is the sole settlement marker; queued-turn envelopes persist at dispatch
 * time, so ordering is sound. Exported for direct unit testing.
 */
export function lastTurnUnsettled(payloads: string[]): boolean {
  const types = payloads.map((raw) => {
    try {
      const value = JSON.parse(raw) as unknown;
      return value &&
        typeof value === "object" &&
        typeof (value as { type?: unknown }).type === "string"
        ? (value as { type: string }).type
        : null;
    } catch {
      return null;
    }
  });
  let lastUserIdx = -1;
  for (let i = 0; i < types.length; i++) {
    if (types[i] === "user_message") lastUserIdx = i;
  }
  if (lastUserIdx < 0) return false;
  for (let i = lastUserIdx + 1; i < types.length; i++) {
    if (types[i] === "turn_completed") return false;
  }
  return true;
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
    return { type: "user_message", thread_id: (value as { thread_id?: string }).thread_id ?? "", text, images };
  }
  // Provider events — trust the discriminated union shape. Unknown
  // discriminators fall through to the reducer's `warnOnce` path.
  return value as ProviderRuntimeEvent;
}
