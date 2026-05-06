import type { ProviderRuntimeEvent } from "@/tauri/events";

import {
  applyEvent,
  appendUserMessage,
  createEmptyThreadState,
} from "./reducer";
import type { ChatThreadState } from "./types";

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
      state = appendUserMessage(state, parsed.text);
    } else {
      state = applyEvent(state, parsed);
    }
  }
  // Guarantee post-replay state matches a quiescent thread. The
  // reducer's turn_completed handler already clears `streaming`, but
  // not every transcript ends with one (interrupted resume, partial
  // history, …). Belt and braces.
  return {
    ...state,
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
    return value as UserMessageEnvelope;
  }
  // Provider events — trust the discriminated union shape. Unknown
  // discriminators fall through to the reducer's `warnOnce` path.
  return value as ProviderRuntimeEvent;
}
