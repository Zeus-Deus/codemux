import type { ProviderRuntimeEvent } from "@/tauri/events";

import {
  applyEvent,
  appendUserMessage,
  createEmptyThreadState,
} from "./reducer";
import type { ChatThreadState, UserMessageImage } from "./types";

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
 */
export function replayPayloads(payloads: string[]): ChatThreadState {
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
  const messages = hasStreamingReasoning
    ? state.messages.map((m) =>
        m.kind === "reasoning" && m.streaming ? { ...m, streaming: false } : m,
      )
    : state.messages;
  return {
    ...state,
    messages,
    streaming: false,
    pendingRequestIds: [],
  };
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
